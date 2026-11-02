package io.github.aabolfazl.perfagent

import android.net.LocalServerSocket
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.os.Process
import android.os.SystemClock
import android.util.Log
import io.github.aabolfazl.perfagent.engine.Engine
import io.github.aabolfazl.perfagent.engine.PcapStream
import io.github.aabolfazl.perfagent.engine.SnapshotListener
import io.github.aabolfazl.perfagent.proto.Snapshot
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.IOException
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * Serves the computer over `adb forward`:
 *  - "android-perf-agent": one length-delimited protobuf [Snapshot] per second.
 *  - "android-perf-agent-pcap": a live pcap stream of every packet.
 *
 * Both are abstract unix sockets. Any app on the phone could connect to them, so only adb
 * (the shell user) and root are served.
 */
internal class ControlServer(private val engine: Engine) : SnapshotListener, Closeable {
    private val clients = CopyOnWriteArrayList<StatsClient>()
    private val stats = Listener(STATS_SOCKET) { socket ->
        clients.add(StatsClient(socket))
        markClientSeen()
    }
    private val pcap = Listener(PCAP_SOCKET) { socket -> startPcap(socket) }

    @Volatile
    private var lastClientAt = SystemClock.elapsedRealtime()

    fun start() {
        engine.snapshotListener = this
        stats.start()
        pcap.start()
    }

    fun markClientSeen() {
        lastClientAt = SystemClock.elapsedRealtime()
    }

    fun idleMillis(): Long = if (clients.isNotEmpty()) 0 else SystemClock.elapsedRealtime() - lastClientAt

    override fun onSnapshot(snapshot: Snapshot) {
        if (clients.isEmpty()) return
        markClientSeen()
        val bytes = ByteArrayOutputStream().also { snapshot.writeDelimitedTo(it) }.toByteArray()
        for (client in clients) client.offer(bytes)
    }

    private fun startPcap(socket: LocalSocket) {
        val stream = PcapStream(socket.outputStream) { closed ->
            // Clear the tap only if a newer pcap client hasn't replaced it.
            if (engine.packetTap === closed) engine.packetTap = null
            runCatching { socket.close() }
        }
        (engine.packetTap as? PcapStream)?.close()
        engine.packetTap = stream
    }

    override fun close() {
        engine.snapshotListener = null
        (engine.packetTap as? PcapStream)?.close()
        engine.packetTap = null
        stats.close()
        pcap.close()
        for (client in clients) client.close()
        clients.clear()
    }

    /** Sends the newest snapshot to one client; slow clients skip snapshots instead of queueing. */
    private inner class StatsClient(private val socket: LocalSocket) : Closeable {
        private val pending = AtomicReference<ByteArray?>()
        private val wakeUp = ArrayBlockingQueue<Unit>(1)

        @Volatile
        private var open = true

        private val thread = Thread(::writeLoop, "perf-agent-client").apply {
            isDaemon = true
            start()
        }

        fun offer(bytes: ByteArray) {
            pending.set(bytes)
            wakeUp.offer(Unit)
        }

        private fun writeLoop() {
            try {
                val out = socket.outputStream
                while (open) {
                    wakeUp.poll(1, TimeUnit.SECONDS)
                    val bytes = pending.getAndSet(null) ?: continue
                    out.write(bytes)
                    out.flush()
                }
            } catch (e: IOException) {
                // The computer disconnected.
            } catch (e: InterruptedException) {
                // Closing.
            } finally {
                close()
            }
        }

        override fun close() {
            open = false
            clients.remove(this)
            runCatching { socket.close() }
        }
    }

    /** Accepts connections on one abstract socket, keeping out everything but adb and root. */
    private class Listener(private val name: String, private val onClient: (LocalSocket) -> Unit) : Closeable {
        private val server = LocalServerSocket(name)

        @Volatile
        private var open = true

        private val thread = Thread(::acceptLoop, "perf-agent-$name")

        fun start() {
            thread.isDaemon = true
            thread.start()
        }

        private fun acceptLoop() {
            while (open) {
                val socket = try {
                    server.accept()
                } catch (e: IOException) {
                    if (open) Log.w(TAG, "accept on $name failed", e)
                    break
                }
                if (!open) {
                    runCatching { socket.close() }
                    break
                }
                val uid = runCatching { socket.peerCredentials.uid }.getOrDefault(-1)
                if (uid != SHELL_UID && uid != Process.ROOT_UID) {
                    Log.w(TAG, "Refused a connection to $name from uid $uid")
                    runCatching { socket.close() }
                    continue
                }
                onClient(socket)
            }
        }

        override fun close() {
            if (!open) return
            open = false
            // accept() doesn't return when the server socket is closed, so connect once to wake it.
            runCatching { LocalSocket().use { it.connect(LocalSocketAddress(name)) } }
            runCatching { server.close() }
        }
    }

    companion object {
        const val STATS_SOCKET = "android-perf-agent"
        const val PCAP_SOCKET = "android-perf-agent-pcap"
        private const val SHELL_UID = 2000
    }
}
