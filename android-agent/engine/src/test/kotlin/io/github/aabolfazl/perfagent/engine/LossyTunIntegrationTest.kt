package io.github.aabolfazl.perfagent.engine

import org.junit.AfterClass
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicLong
import kotlin.concurrent.thread
import kotlin.random.Random
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertTrue

/** Drops a share of the packets in both directions, to exercise the retransmission paths. */
private class LossyTun(private val inner: TunDevice, private val lossRate: Double) : TunDevice {
    private val readRandom = Random(1)
    private val writeRandom = Random(2)
    val droppedReads = AtomicLong()
    val droppedWrites = AtomicLong()

    override fun read(buffer: ByteArray): Int {
        while (true) {
            val n = inner.read(buffer)
            if (n <= 0 || readRandom.nextDouble() >= lossRate) return n
            droppedReads.incrementAndGet()
        }
    }

    override fun write(buffer: ByteArray, offset: Int, length: Int) {
        if (writeRandom.nextDouble() < lossRate) {
            droppedWrites.incrementAndGet()
            return
        }
        inner.write(buffer, offset, length)
    }
}

class LossyTunIntegrationTest {
    companion object {
        private val localhost: InetAddress = InetAddress.getByName("127.0.0.1")
        private var tun: LinuxTun? = null
        private var lossy: LossyTun? = null
        private var engine: Engine? = null

        @BeforeClass
        @JvmStatic
        fun setUp() {
            if (System.getenv("ENGINE_DEBUG") != null) {
                val handler = java.util.logging.ConsoleHandler().apply { level = java.util.logging.Level.ALL }
                Engine.log.addHandler(handler)
                Engine.log.level = java.util.logging.Level.ALL
            }
            val device = try {
                LinuxTun("apengine1", "198.18.1.1", 24)
            } catch (e: Throwable) {
                null
            }
            assumeTrue("needs root and /dev/net/tun", device != null)
            tun = device
            lossy = LossyTun(device!!, lossRate = (System.getenv("ENGINE_LOSS") ?: "0.02").toDouble())
            val platform = object : Platform {
                override fun protect(socket: Socket) = true
                override fun protect(socket: DatagramSocket) = true
                override fun upstreamAddress(destination: InetSocketAddress) = InetSocketAddress(localhost, destination.port)
                override fun ownerUid(protocol: Int, local: InetSocketAddress, remote: InetSocketAddress) = 1
                override fun appName(uid: Int) = "app"
                override fun resolveDns(query: ByteArray, reply: (ByteArray?) -> Unit) = reply(null)
            }
            engine = Engine(lossy!!, platform).apply {
                System.getenv("ENGINE_PCAP")?.let { packetTap = PcapStream(java.io.FileOutputStream(it)) }
                start()
            }
        }

        @AfterClass
        @JvmStatic
        fun tearDown() {
            engine?.stop()
            tun?.close()
        }
    }

    @Test(timeout = 90_000)
    fun transferSurvivesPacketLoss() {
        val download = Random(3).nextBytes(2 * 1024 * 1024)
        val upload = Random(4).nextBytes(512 * 1024)
        var received = ByteArray(0)

        ServerSocket(0, 1, localhost).use { server ->
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
            Socket().use { client ->
                client.connect(InetSocketAddress("198.18.1.20", server.localPort), 10_000)
                client.soTimeout = 60_000
                val writer = thread {
                    client.getOutputStream().write(upload)
                    client.shutdownOutput()
                }
                assertContentEquals(download, client.getInputStream().readAllBytes())
                writer.join()
            }
            serverThread.join()
        }
        assertContentEquals(upload, received)
        assertTrue(lossy!!.droppedReads.get() > 0 && lossy!!.droppedWrites.get() > 0, "nothing was dropped")
        println("dropped ${lossy!!.droppedReads.get()} packets from the app and ${lossy!!.droppedWrites.get()} to it")
    }
}
