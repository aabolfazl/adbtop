package io.github.aabolfazl.perfagent.engine

import java.io.ByteArrayOutputStream
import java.net.ServerSocket
import java.net.Socket
import javax.net.ssl.SNIHostName
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import kotlin.concurrent.thread
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull

internal fun dnsQuery(id: Int, name: String): ByteArray {
    val out = ByteArrayOutputStream()
    out.write(byteArrayOf((id ushr 8).toByte(), id.toByte(), 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0))
    for (label in name.split('.')) {
        out.write(label.length)
        out.write(label.toByteArray())
    }
    out.write(byteArrayOf(0, 0, 1, 0, 1))
    return out.toByteArray()
}

/** A response with a CNAME followed by A records, using name compression like real servers. */
internal fun dnsAnswer(query: ByteArray, vararg addresses: ByteArray): ByteArray {
    val out = ByteArrayOutputStream()
    out.write(query, 0, 2)
    out.write(byteArrayOf(0x81.toByte(), 0x80.toByte(), 0, 1, 0, (addresses.size + 1).toByte(), 0, 0, 0, 0))
    out.write(query, 12, query.size - 12)
    // CNAME pointing at the question name (offset 12) with target "cdn" + pointer to the question name.
    out.write(byteArrayOf(0xC0.toByte(), 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 6, 3) + "cdn".toByteArray() + byteArrayOf(0xC0.toByte(), 12))
    for (address in addresses) {
        out.write(byteArrayOf(0xC0.toByte(), 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4))
        out.write(address)
    }
    return out.toByteArray()
}

class InspectTest {
    @Test
    fun parsesDnsQueryAndAnswer() {
        val query = dnsQuery(0x1234, "www.example.com")
        val q = Dns.parse(query, 0, query.size)!!
        assertEquals("www.example.com", q.name)
        assertEquals(false, q.isResponse)

        val answer = dnsAnswer(query, byteArrayOf(1, 2, 3, 4), byteArrayOf(5, 6, 7, 8))
        val a = Dns.parse(answer, 0, answer.size)!!
        assertEquals(0x1234, a.id)
        assertEquals(true, a.isResponse)
        assertEquals("www.example.com", a.name)
        assertEquals(listOf(listOf<Byte>(1, 2, 3, 4), listOf<Byte>(5, 6, 7, 8)), a.addresses.map { it.toList() })
    }

    @Test
    fun rejectsDnsPointerLoops() {
        val bad = byteArrayOf(0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0.toByte(), 12)
        assertNull(Dns.parse(bad, 0, bad.size))
    }

    @Test
    fun findsHttpHost() {
        val request = "GET /a HTTP/1.1\r\nUser-Agent: x\r\nhost: api.example.com:8080\r\n\r\n".toByteArray()
        val result = assertIs<HostResult.Found>(HostSniffer.sniff(request, request.size))
        assertEquals("api.example.com", result.host)
        assertEquals(HostSniffer.SOURCE_HTTP, result.source)

        val partial = "GET /a HTTP/1.1\r\nHost: api.exa".toByteArray()
        assertIs<HostResult.NeedMore>(HostSniffer.sniff(partial, partial.size))
        val notHttp = "SSH-2.0-OpenSSH".toByteArray()
        assertIs<HostResult.NotFound>(HostSniffer.sniff(notHttp, notHttp.size))
    }

    @Test
    fun findsTlsServerNameInARealClientHello() {
        val hello = captureClientHello("sni.example.test")
        val result = assertIs<HostResult.Found>(HostSniffer.sniff(hello, hello.size))
        assertEquals("sni.example.test", result.host)
        assertEquals(HostSniffer.SOURCE_TLS, result.source)
        // Split in two TCP segments, the first part alone is not enough.
        assertIs<HostResult.NeedMore>(HostSniffer.sniff(hello, 100))
    }

    /** Lets the JDK's TLS client write a ClientHello to a plain socket and returns the bytes. */
    private fun captureClientHello(host: String): ByteArray {
        ServerSocket(0).use { server ->
            val client = thread {
                runCatching {
                    val socket = SSLContext.getDefault().socketFactory.createSocket("127.0.0.1", server.localPort) as SSLSocket
                    socket.sslParameters = socket.sslParameters.apply { serverNames = listOf(SNIHostName(host)) }
                    socket.soTimeout = 2000
                    socket.startHandshake()
                }
            }
            val accepted: Socket = server.accept()
            accepted.soTimeout = 2000
            val buffer = ByteArray(HostSniffer.MAX_BYTES)
            var length = 0
            // Read until the whole first TLS record is here.
            while (length < 5 || length < 5 + u16(buffer, 3)) {
                val n = accepted.getInputStream().read(buffer, length, buffer.size - length)
                if (n < 0) break
                length += n
            }
            accepted.close()
            client.join()
            return buffer.copyOf(length)
        }
    }
}
