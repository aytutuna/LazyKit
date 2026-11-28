package app.alazy.linktests

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import app.alazy.link.Tls
import uniffi.lazy_link_ffi.AppAttester
import uniffi.lazy_link_ffi.AppProof
import uniffi.lazy_link_ffi.PushPlatform
import uniffi.lazy_link_ffi.Relay
import uniffi.lazy_link_ffi.RelayFailure
import java.net.URI
import java.security.KeyPairGenerator
import java.security.SecureRandom
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.concurrent.atomic.AtomicInteger

// The desktop proof the relay asks of a computer: a key this install made, signing the group and the challenge. The
// relay takes any install that holds a key, which is all an emulator can show.
class InstallKey : AppAttester {
    private val pair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
    val calls = AtomicInteger()

    override suspend fun attest(group: String, challenge: String): AppProof {
        calls.incrementAndGet()
        val message = "lazy relay install v1\u0000$group\u0000$challenge".toByteArray()
        val sig = Signature.getInstance("SHA256withECDSA").run {
            initSign(pair.private)
            update(message)
            sign()
        }
        val point = (pair.public as ECPublicKey).w
        fun fixed(v: java.math.BigInteger, n: Int) = v.toByteArray().let { b -> ByteArray(n).also { out ->
            val src = if (b.size > n) b.copyOfRange(b.size - n, b.size) else b
            System.arraycopy(src, 0, out, n - src.size, src.size)
        } }
        return AppProof.Desktop(byteArrayOf(4) + fixed(point.affineX, 32) + fixed(point.affineY, 32), derToRaw(sig))
    }

    // Java signs in DER; the relay wants r then s, 32 bytes each.
    private fun derToRaw(der: ByteArray): ByteArray {
        var at = 2 + if (der[1].toInt() and 0x80 != 0) 1 else 0
        fun int(): ByteArray {
            val len = der[at + 1].toInt()
            val v = der.copyOfRange(at + 2, at + 2 + len)
            at += 2 + len
            val t = v.dropWhile { it == 0.toByte() }.toByteArray()
            return ByteArray(32 - t.size) + t
        }
        val r = int()
        return r + int()
    }
}

@RunWith(AndroidJUnit4::class)
class RelayTest {
    // The development relay under `wrangler dev --local-protocol https`, reached from the emulator at 10.0.2.2. Its
    // certificate authority is in the device's user certificate store (scripts/link-android.sh).
    private val url = URI(InstrumentationRegistry.getArguments().getString("relayUrl") ?: error("relayUrl is not set"))
    private val base = url.toString().trimEnd('/')

    // The platform verifier needs the app's context before the first connection.
    @Before
    fun startTls() = Tls.init(InstrumentationRegistry.getInstrumentation().targetContext)

    private fun id() = ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
    private fun room() = ByteArray(32).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
    private fun relay(group: String = id(), attester: AppAttester? = null) = Relay(base, group, id(), attester)

    @Test
    fun groupCallsRunOverRustTls() = runBlocking {
        val s = relay().status()
        assertEquals(0UL, s.used)
        assertEquals(5_000_000_000UL, s.cap)
        assertFalse(s.capped)
    }

    @Test
    fun aNewGroupNeedsItsProofAndNothingFlowsWithout() = runBlocking {
        try {
            relay().connect(room())
            throw AssertionError("a room was opened without a proof")
        } catch (_: RelayFailure.AttestationRequired) {
        }
        try {
            relay().registerPush(PushPlatform.FCM, "token")
            throw AssertionError("a push token was kept without a proof")
        } catch (_: RelayFailure.AttestationRequired) {
        }
    }

    @Test
    fun twoDevicesTalkThroughTheRelay() = runBlocking {
        val group = id()
        val room = room()
        val hostKey = InstallKey()
        val host = relay(group, hostKey).host(room)
        assertEquals(1, hostKey.calls.get())

        val guestKey = InstallKey()
        val guestSide = relay(group, guestKey).connect(room)
        val hostSide = host.accept()
        assertEquals(0, guestKey.calls.get())

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
    fun anotherGroupCannotUseTheRoom() = runBlocking {
        val room = room()
        val host = relay(id(), InstallKey()).host(room)
        assertTrue(runCatching { relay(id(), InstallKey()).connect(room) }.exceptionOrNull() is RelayFailure.WrongGroup)
        java.lang.ref.Reference.reachabilityFence(host)
    }
}
