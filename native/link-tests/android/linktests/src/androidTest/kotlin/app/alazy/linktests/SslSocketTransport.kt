package app.alazy.linktests

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.lazy_link_ffi.LinkException
import uniffi.lazy_link_ffi.SecureTransport
import java.io.IOException
import java.net.InetSocketAddress
import java.net.Socket
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.TrustManager
import javax.net.ssl.X509TrustManager

// What an Android app gives the relay client: SSLSocket with the system's trust store and host name check. The tests
// talk to a development relay with a self-signed certificate, so they can ask it to accept any certificate.
class SslSocketTransport(private val acceptAnyCertificate: Boolean) : SecureTransport {
    private val sockets = ConcurrentHashMap<ULong, SSLSocket>()
    private val last = AtomicLong()

    val open get() = sockets.size

    private val context: SSLContext = SSLContext.getInstance("TLS").apply {
        val trustAll = object : X509TrustManager {
            override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {}
            override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {}
            override fun getAcceptedIssuers() = emptyArray<X509Certificate>()
        }
        init(null, if (acceptAnyCertificate) arrayOf<TrustManager>(trustAll) else null, SecureRandom())
    }

    override suspend fun connect(host: String, port: UShort, serverName: String): ULong = withContext(Dispatchers.IO) {
        val plain = Socket()
        try {
            plain.connect(InetSocketAddress(host, port.toInt()), 15_000)
            val ssl = context.socketFactory.createSocket(plain, serverName, port.toInt(), true) as SSLSocket
            ssl.startHandshake()
            if (!acceptAnyCertificate && !HttpsURLConnection.getDefaultHostnameVerifier().verify(serverName, ssl.session)) {
                throw IOException("the certificate is not for $serverName")
            }
            val id = last.incrementAndGet().toULong()
            sockets[id] = ssl
            id
        } catch (e: IOException) {
            plain.close()
            throw LinkException.Unexpected(e.message ?: "connect failed")
        }
    }

    override suspend fun read(connection: ULong, max: UInt): ByteArray = withContext(Dispatchers.IO) {
        val socket = sockets[connection] ?: return@withContext ByteArray(0)
        try {
            val buf = ByteArray(max.toInt())
            val n = socket.inputStream.read(buf)
            if (n <= 0) ByteArray(0) else buf.copyOf(n)
        } catch (e: IOException) {
            ByteArray(0)
        }
    }

    override suspend fun write(connection: ULong, data: ByteArray) = withContext(Dispatchers.IO) {
        val socket = sockets[connection] ?: throw LinkException.Unexpected("closed")
        try {
            socket.outputStream.write(data)
            socket.outputStream.flush()
        } catch (e: IOException) {
            throw LinkException.Unexpected(e.message ?: "write failed")
        }
    }

    override suspend fun close(connection: ULong) = withContext(Dispatchers.IO) {
        sockets.remove(connection)?.close()
        Unit
    }

    override fun peerSpki(connection: ULong): ByteArray? =
        (sockets[connection]?.session?.peerCertificates?.firstOrNull() as? X509Certificate)?.publicKey?.encoded
}
