package io.github.aabolfazl.perfagent.engine

import io.github.aabolfazl.perfagent.proto.ConnectionState
import io.github.aabolfazl.perfagent.proto.HostSource
import io.github.aabolfazl.perfagent.proto.Snapshot
import org.junit.AfterClass
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import java.io.ByteArrayOutputStream
import java.net.ConnectException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.SNIHostName
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import kotlin.concurrent.thread
import kotlin.random.Random
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * Runs real Linux traffic through a TUN device into the engine, the same way Android hands
 * app traffic to a VpnService. Addresses in 198.18.0.0/24 are routed into the TUN device and
 * forwarded to servers on 127.0.0.1. Skipped unless running as root with /dev/net/tun.
 */
class TunIntegrationTest {
    companion object {
        private const val UID = 10123
        private val localhost: InetAddress = InetAddress.getByName("127.0.0.1")
        private val latest = AtomicReference<Snapshot>()
        private var tun: LinuxTun? = null
        private var engine: Engine? = null

        private val platform = object : Platform {
            override fun protect(socket: Socket) = true
            override fun protect(socket: DatagramSocket) = true

            override fun upstreamAddress(destination: InetSocketAddress): InetSocketAddress =
                if (destination.address.hostAddress.startsWith("198.18.0.")) InetSocketAddress(localhost, destination.port)
                else destination

            override fun ownerUid(protocol: Int, local: InetSocketAddress, remote: InetSocketAddress) = UID
            override fun appName(uid: Int) = "com.example.test"

            override fun resolveDns(query: ByteArray, reply: (ByteArray?) -> Unit) {
                val name = Dns.parse(query, 0, query.size)?.name
                thread {
                    if (name == "fail.test") reply(null)
                    else reply(dnsAnswer(query, byteArrayOf(198.toByte(), 18, 0, 25)))
                }
            }
        }

        @BeforeClass
        @JvmStatic
        fun setUp() {
            val device = try {
                LinuxTun("apengine0", "198.18.0.1", 24)
            } catch (e: Throwable) {
                null
            }
            assumeTrue("needs root and /dev/net/tun", device != null)
            tun = device
            engine = Engine(
                device!!, platform,
                EngineConfig(dnsServer = InetAddress.getByName("198.18.0.2") as java.net.Inet4Address, snapshotIntervalMs = 200),
            ).apply {
                snapshotListener = SnapshotListener { latest.set(it) }
                start()
            }
        }

        @AfterClass
        @JvmStatic
        fun tearDown() {
            engine?.stop()
            assertEquals(false, engine?.isRunning ?: false)
            tun?.close()
        }

        private fun connect(ip: String, port: Int): Socket =
            Socket().apply {
                connect(InetSocketAddress(ip, port), 5000)
                soTimeout = 20_000
            }

        private fun waitForSnapshot(what: String, condition: (Snapshot) -> Boolean): Snapshot {
            val deadline = System.currentTimeMillis() + 5000
            while (System.currentTimeMillis() < deadline) {
                latest.get()?.let { if (condition(it)) return it }
                Thread.sleep(50)
            }
            throw AssertionError("snapshot never showed $what; last: ${latest.get()}")
        }

        private fun ip(snapshotIp: com.google.protobuf.ByteString) =
            InetAddress.getByAddress(snapshotIp.toByteArray()).hostAddress
    }

    @Test(timeout = 60_000)
    fun largeTransferInBothDirections() {
        val download = Random(1).nextBytes(8 * 1024 * 1024)
        val upload = Random(2).nextBytes(1024 * 1024)
        var received = ByteArray(0)

        ServerSocket(0, 50, localhost).use { server ->
            val serverThread = thread {
                server.accept().use { socket ->
                    val writer = thread {
                        socket.getOutputStream().write(download)
                        socket.shutdownOutput()
                    }
                    received = socket.getInputStream().readAllBytes()
                    writer.join()
                }
            }
            connect("198.18.0.20", server.localPort).use { client ->
                val writer = thread {
                    client.getOutputStream().write(upload)
                    client.shutdownOutput()
                }
                val got = client.getInputStream().readAllBytes()
                writer.join()
                assertContentEquals(download, got)
            }
            serverThread.join()
        }
        assertContentEquals(upload, received)
    }

    @Test(timeout = 60_000)
    fun appThatStopsReadingFillsItsWindowAndResumes() {
        val download = Random(5).nextBytes(4 * 1024 * 1024)
        ServerSocket(0, 1, localhost).use { server ->
            val serverThread = thread { server.accept().use { it.getOutputStream().write(download) } }
            connect("198.18.0.20", server.localPort).use { client ->
                client.receiveBufferSize = 32 * 1024
                // While the app doesn't read, its receive window drops to zero and the engine has to wait and probe.
                Thread.sleep(2500)
                assertContentEquals(download, client.getInputStream().readAllBytes())
            }
            serverThread.join()
        }
    }

    @Test(timeout = 60_000)
    fun manyParallelConnections() {
        ServerSocket(0, 100, localhost).use { server ->
            val acceptor = thread {
                repeat(32) {
                    val socket = server.accept()
                    thread {
                        socket.use { s ->
                            val buffer = ByteArray(8192)
                            while (true) {
                                val n = s.getInputStream().read(buffer)
                                if (n < 0) break
                                s.getOutputStream().write(buffer, 0, n)
                            }
                        }
                    }
                }
            }
            val failures = java.util.concurrent.ConcurrentLinkedQueue<Throwable>()
            val clients = (0 until 32).map { i ->
                thread {
                    try {
                        val data = Random(i).nextBytes(256 * 1024)
                        connect("198.18.0.20", server.localPort).use { client ->
                            val writer = thread {
                                client.getOutputStream().write(data)
                                client.shutdownOutput()
                            }
                            val echoed = client.getInputStream().readAllBytes()
                            writer.join()
                            assertContentEquals(data, echoed)
                        }
                    } catch (e: Throwable) {
                        failures.add(e)
                    }
                }
            }
            clients.forEach { it.join() }
            acceptor.join()
            failures.firstOrNull()?.let { throw it }
        }
    }

    @Test(timeout = 20_000)
    fun refusedConnectionFailsFast() {
        val closedPort = ServerSocket(0, 1, localhost).use { it.localPort }
        val started = System.currentTimeMillis()
        assertFailsWith<ConnectException> { connect("198.18.0.21", closedPort) }
        assertTrue(System.currentTimeMillis() - started < 3000)

        val snapshot = waitForSnapshot("the failed connection") { s ->
            s.connectionsList.any { it.remotePort == closedPort && it.state == ConnectionState.FAILED }
        }
        val failed = snapshot.connectionsList.first { it.remotePort == closedPort }
        assertTrue(failed.error.isNotEmpty())
    }

    @Test(timeout = 20_000)
    fun halfCloseFromTheApp() {
        ServerSocket(0, 1, localhost).use { server ->
            val serverThread = thread {
                server.accept().use { s ->
                    val request = s.getInputStream().readAllBytes()
                    s.getOutputStream().write("got ${request.size} bytes".toByteArray())
                }
            }
            connect("198.18.0.20", server.localPort).use { client ->
                client.getOutputStream().write(ByteArray(5000))
                client.shutdownOutput()
                assertEquals("got 5000 bytes", String(client.getInputStream().readAllBytes()))
            }
            serverThread.join()
            waitForSnapshot("the closed connection") { s ->
                s.connectionsList.any { it.remotePort == server.localPort && it.state == ConnectionState.CLOSED && it.error.isEmpty() }
            }
        }
    }

    @Test(timeout = 20_000)
    fun udpRoundTrip() {
        DatagramSocket(0, localhost).use { echo ->
            val echoThread = thread {
                val buffer = ByteArray(2048)
                runCatching {
                    while (true) {
                        val packet = DatagramPacket(buffer, buffer.size)
                        echo.receive(packet)
                        echo.send(DatagramPacket(packet.data, packet.length, packet.socketAddress))
                    }
                }
            }
            DatagramSocket().use { client ->
                client.soTimeout = 3000
                for (size in listOf(1, 100, 1000, 1400)) {
                    val data = Random(size).nextBytes(size)
                    client.send(DatagramPacket(data, size, InetAddress.getByName("198.18.0.30"), echo.localPort))
                    val reply = DatagramPacket(ByteArray(2048), 2048)
                    client.receive(reply)
                    assertContentEquals(data, reply.data.copyOf(reply.length))
                    assertEquals("198.18.0.30", reply.address.hostAddress)
                }
            }
            echo.close()
            echoThread.join()
        }
    }

    @Test(timeout = 20_000)
    fun dnsAnswersNameTheirConnections() {
        val query = dnsQuery(42, "example.test")
        val answer = DatagramSocket().use { s ->
            s.soTimeout = 3000
            s.send(DatagramPacket(query, query.size, InetAddress.getByName("198.18.0.2"), 53))
            val reply = DatagramPacket(ByteArray(1500), 1500)
            s.receive(reply)
            Dns.parse(reply.data, 0, reply.length)!!
        }
        assertEquals(42, answer.id)
        assertEquals(listOf("198.18.0.25"), answer.addresses.map { InetAddress.getByAddress(it).hostAddress })

        ServerSocket(0, 1, localhost).use { server ->
            val serverThread = thread { server.accept().close() }
            connect("198.18.0.25", server.localPort).close()
            serverThread.join()
            val snapshot = waitForSnapshot("a connection named by DNS") { s ->
                s.connectionsList.any { it.remotePort == server.localPort && it.host == "example.test" }
            }
            assertEquals(HostSource.DNS, snapshot.connectionsList.first { it.remotePort == server.localPort }.hostSource)
            assertTrue(snapshot.dnsList.any { it.name == "example.test" && !it.failed })
        }
    }

    @Test(timeout = 20_000)
    fun failedDnsGetsServfail() {
        val query = dnsQuery(7, "fail.test")
        DatagramSocket().use { s ->
            s.soTimeout = 3000
            s.send(DatagramPacket(query, query.size, InetAddress.getByName("198.18.0.2"), 53))
            val reply = DatagramPacket(ByteArray(1500), 1500)
            s.receive(reply)
            val message = Dns.parse(reply.data, 0, reply.length)!!
            assertEquals(7, message.id)
            assertTrue(message.isResponse)
            assertEquals(2, message.responseCode)
        }
    }

    @Test(timeout = 20_000)
    fun httpHostAndTlsServerNameAreRecorded() {
        ServerSocket(0, 5, localhost).use { server ->
            val serverThread = thread {
                repeat(2) {
                    server.accept().use { s ->
                        s.soTimeout = 3000
                        runCatching { s.getInputStream().read(ByteArray(4096)) }
                    }
                }
            }
            connect("198.18.0.26", server.localPort).use {
                it.getOutputStream().write("GET / HTTP/1.1\r\nHost: www.example.test\r\n\r\n".toByteArray())
                it.getInputStream().read()
            }
            runCatching {
                val raw = connect("198.18.0.27", server.localPort)
                val tls = SSLContext.getDefault().socketFactory.createSocket(raw, "198.18.0.27", server.localPort, true) as SSLSocket
                tls.sslParameters = tls.sslParameters.apply { serverNames = listOf(SNIHostName("sni.example.test")) }
                tls.use { it.startHandshake() }
            }
            serverThread.join()

            val snapshot = waitForSnapshot("HTTP and TLS hosts") { s ->
                s.connectionsList.any { it.host == "www.example.test" } && s.connectionsList.any { it.host == "sni.example.test" }
            }
            assertEquals(HostSource.HTTP_HOST, snapshot.connectionsList.first { it.host == "www.example.test" }.hostSource)
            val tlsConnection = snapshot.connectionsList.first { it.host == "sni.example.test" }
            assertEquals(HostSource.TLS_SNI, tlsConnection.hostSource)
            assertEquals("198.18.0.27", ip(tlsConnection.remoteIp))
        }
    }

    @Test(timeout = 30_000)
    fun snapshotCountsTrafficPerApp() {
        val before = latest.get()?.appsList?.firstOrNull { it.uid == UID }?.rxBytes ?: 0
        ServerSocket(0, 1, localhost).use { server ->
            val serverThread = thread { server.accept().use { it.getOutputStream().write(ByteArray(300_000)) } }
            connect("198.18.0.20", server.localPort).use { assertEquals(300_000, it.getInputStream().readAllBytes().size) }
            serverThread.join()
        }
        val snapshot = waitForSnapshot("the downloaded bytes") { s ->
            (s.appsList.firstOrNull { it.uid == UID }?.rxBytes ?: 0) >= before + 300_000
        }
        val app = snapshot.appsList.first { it.uid == UID }
        assertEquals("com.example.test", app.name)
        assertTrue(app.txBytes > 0)
        assertTrue(snapshot.stats.packetsFromApps > 0 && snapshot.stats.packetsToApps > 0)
        assertTrue(snapshot.startedMs in 1..snapshot.timeMs)
    }

    @Test(timeout = 30_000)
    fun pcapContainsValidPackets() {
        val out = ByteArrayOutputStream()
        val pcap = PcapStream(out)
        engine!!.packetTap = pcap
        ServerSocket(0, 1, localhost).use { server ->
            val serverThread = thread { server.accept().use { it.getOutputStream().write(ByteArray(50_000)) } }
            connect("198.18.0.28", server.localPort).use { it.getInputStream().readAllBytes() }
            serverThread.join()
        }
        Thread.sleep(300)
        engine!!.packetTap = null
        pcap.close()

        val bytes = out.toByteArray()
        val le = java.nio.ByteBuffer.wrap(bytes).order(java.nio.ByteOrder.LITTLE_ENDIAN)
        assertEquals(0xA1B2C3D4.toInt(), le.getInt(0))
        assertEquals(Pcap.LINKTYPE_RAW, le.getInt(20))

        var pos = 24
        var packets = 0
        var sawSyn = false
        while (pos + 16 <= bytes.size) {
            val length = le.getInt(pos + 8)
            val packet = bytes.copyOfRange(pos + 16, pos + 16 + length)
            assertTrue(checksumsValid(packet), "bad checksum in packet $packets")
            val ip = IpPacket.parse(packet, packet.size)!!
            if (ip.protocol == PROTO_TCP && ip.dst == u32(byteArrayOf(198.toByte(), 18, 0, 28), 0)) {
                if (TcpSegment.parse(ip)!!.has(TCP_SYN)) sawSyn = true
            }
            pos += 16 + length
            packets++
        }
        assertEquals(bytes.size, pos)
        assertTrue(packets > 40, "only $packets packets")
        assertTrue(sawSyn)
    }
}
