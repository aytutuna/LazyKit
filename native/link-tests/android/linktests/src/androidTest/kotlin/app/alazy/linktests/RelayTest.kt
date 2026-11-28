package app.alazy.linktests

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.lazy_link_ffi.Account
import uniffi.lazy_link_ffi.AccountFailure
import uniffi.lazy_link_ffi.Bot
import uniffi.lazy_link_ffi.GroupPlatform
import uniffi.lazy_link_ffi.Login
import uniffi.lazy_link_ffi.PushPlatform
import uniffi.lazy_link_ffi.Relay
import uniffi.lazy_link_ffi.RelayFailure
import uniffi.lazy_link_ffi.Session
import uniffi.lazy_link_ffi.trustDevelopmentRoot
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.security.SecureRandom

@RunWith(AndroidJUnit4::class)
class RelayTest {
    // The development relay under `wrangler dev --local-protocol https` and its mail sink, reached from the emulator at
    // 10.0.2.2. Android checks the relay against bundled roots only, so the relay's development authority (an asset,
    // put there by scripts/link-android.sh) is added by hand.
    private val args = InstrumentationRegistry.getArguments()
    private val base = (args.getString("relayUrl") ?: error("relayUrl is not set")).trimEnd('/')
    private val accountBase = base.replace("/relay/", "/account/")
    private val sink = args.getString("mailSink") ?: "http://10.0.2.2:8790"
    private val password = "correct horse battery"

    @Before
    fun trustTheDevelopmentAuthority() {
        val pem = InstrumentationRegistry.getInstrumentation().context.assets.open("dev_ca.pem").bufferedReader().readText()
        val body = pem.lines().filter { !it.startsWith("-----") }.joinToString("")
        trustDevelopmentRoot(java.util.Base64.getMimeDecoder().decode(body))
    }

    private fun id() = ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
    private fun room() = ByteArray(32).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
    private fun who(device: String) = Login("lazysend", device, null)

    // The newest code the service mailed to an address, read from the dev sink.
    private suspend fun codeFor(email: String): String {
        repeat(50) {
            val reply = withContext(Dispatchers.IO) {
                val c = URL("$sink/code?to=${URLEncoder.encode(email, "UTF-8")}").openConnection() as HttpURLConnection
                try {
                    if (c.responseCode == 200) c.inputStream.bufferedReader().readText().trim() else null
                } finally {
                    c.disconnect()
                }
            }
            if (reply != null) return reply
            delay(100)
        }
        error("no mail came for $email")
    }

    private inner class Person(val email: String, val device: String, val session: Session) {
        fun relay(group: String, device: String = this.device, session: Session = this.session) =
            Relay.newAt(base, group, device, session.token, null)
    }

    // A new account, signed up through the service with the code from the dev sink.
    private suspend fun signUp(): Person {
        val email = "${id().take(12)}@example.com"
        val device = id()
        val account = Account.newAt(accountBase, null)
        account.signup(email, Bot.Turnstile("dev"))
        return Person(email, device, account.confirmSignup(email, codeFor(email), password, who(device)))
    }

    private suspend fun signIn(email: String, device: String) =
        Account.newAt(accountBase, null).signinPassword(email, password, who(device))

    @Test
    fun aSignedInAccountClaimsAGroupAndSeesItsAllowance() = runBlocking {
        val person = signUp()
        val relay = person.relay(id())
        assertEquals(36, relay.claim(GroupPlatform.DESKTOP).accountToken.length)
        val s = relay.status()
        assertEquals(0UL, s.used)
        assertEquals(5_000_000_000UL, s.cap)
        assertFalse(s.capped)
        assertTrue(person.session.accountJson.contains("\"email\":\"${person.email}\""))
    }

    @Test
    fun wrongPasswordsAndMissingSessionsAreRefusedByName() = runBlocking {
        val person = signUp()
        try {
            Account.newAt(accountBase, null).signinPassword(person.email, "not the password", who(id()))
            throw AssertionError("a wrong password was accepted")
        } catch (_: AccountFailure.WrongCredentials) {
        }
        try {
            Relay.newAt(base, id(), person.device, "", null).status()
            throw AssertionError("a request without a session was answered")
        } catch (_: RelayFailure.SignIn) {
        }
    }

    @Test
    fun aPhoneGroupNeedsItsProofAndNothingFlowsWithout() = runBlocking {
        val relay = signUp().relay(id())
        relay.claim(GroupPlatform.APPLE)
        try {
            relay.connect(room())
            throw AssertionError("a room was opened without a proof")
        } catch (_: RelayFailure.AttestationRequired) {
        }
        try {
            relay.registerPush(PushPlatform.FCM, "token")
            throw AssertionError("a push token was kept without a proof")
        } catch (_: RelayFailure.AttestationRequired) {
        }
    }

    @Test
    fun twoDevicesTalkThroughTheRelay() = runBlocking {
        val person = signUp()
        val group = id()
        val room = room()
        person.relay(group).claim(GroupPlatform.DESKTOP)
        val host = person.relay(group).host(room)
        val laptop = id()
        val guestSide = person.relay(group, laptop, signIn(person.email, laptop)).connect(room)
        val hostSide = host.accept()

        val up = ByteArray(300_000).also { SecureRandom().nextBytes(it) }
        val down = ByteArray(200_000).also { SecureRandom().nextBytes(it) }
        val send = async {
            up.asList().chunked(50_000).forEach { guestSide.write(it.toByteArray()) }
            guestSide.shutdown()
        }
        var got = ByteArray(0)
        while (got.size < up.size) got += hostSide.read(65_536u)
        send.await()
        assertArrayEquals(up, got)
        assertEquals(0, hostSide.read(1024u).size)

        hostSide.write(down)
        hostSide.shutdown()
        var back = ByteArray(0)
        while (true) {
            val part = guestSide.read(65_536u)
            if (part.isEmpty()) break
            back += part
        }
        assertArrayEquals(down, back)
        guestSide.finish()
        hostSide.finish()
    }

    @Test
    fun anotherAccountCannotUseTheGroup() = runBlocking {
        val mine = signUp()
        val group = id()
        mine.relay(group).claim(GroupPlatform.DESKTOP)
        val other = signUp()
        try {
            other.relay(group).status()
            throw AssertionError("another account used the group")
        } catch (_: RelayFailure.WrongAccount) {
        }
        // A session made for another device does not speak for this one.
        try {
            mine.relay(group, id()).host(room())
            throw AssertionError("a session was used on another device")
        } catch (_: RelayFailure.WrongDevice) {
        }
    }
}
