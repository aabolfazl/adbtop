package io.github.aabolfazl.perfagent.engine

import java.io.Closeable
import java.io.IOException
import java.io.OutputStream
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/** Classic pcap format with raw IP packets (LINKTYPE_RAW), which Wireshark opens directly. */
object Pcap {
    const val LINKTYPE_RAW = 101
    const val SNAP_LENGTH = 65535

    fun fileHeader(): ByteArray {
        val header = ByteArray(24)
        putLe32(header, 0, 0xA1B2C3D4.toInt())
        putLe16(header, 4, 2)
        putLe16(header, 6, 4)
        putLe32(header, 16, SNAP_LENGTH)
        putLe32(header, 20, LINKTYPE_RAW)
        return header
    }

    /** One record: 16-byte header followed by the packet. */
    fun record(data: ByteArray, offset: Int, length: Int, timeMicros: Long): ByteArray {
        val captured = minOf(length, SNAP_LENGTH)
        val record = ByteArray(16 + captured)
        putLe32(record, 0, (timeMicros / 1_000_000).toInt())
        putLe32(record, 4, (timeMicros % 1_000_000).toInt())
        putLe32(record, 8, captured)
        putLe32(record, 12, length)
        System.arraycopy(data, offset, record, 16, captured)
        return record
    }

    private fun putLe16(b: ByteArray, o: Int, v: Int) {
        b[o] = v.toByte()
        b[o + 1] = (v ushr 8).toByte()
    }

    private fun putLe32(b: ByteArray, o: Int, v: Int) {
        putLe16(b, o, v)
        putLe16(b, o + 2, v ushr 16)
    }
}

/**
 * Streams packets as pcap to [out] on its own thread, so a slow reader never blocks the engine.
 * Packets are dropped (and counted) once more than [maxQueuedBytes] are waiting.
 */
class PcapStream(
    private val out: OutputStream,
    private val maxQueuedBytes: Long = 8L * 1024 * 1024,
    /** Called on the writer thread once the stream has ended for any reason. */
    private val onClosed: (PcapStream) -> Unit = {},
) : PacketTap, Closeable {
    private val queue = LinkedBlockingQueue<ByteArray>()
    private val queuedBytes = AtomicLong()
    private val writer = Thread(::writeLoop, "perf-agent-pcap")

    @Volatile
    private var open = true

    val dropped = AtomicLong()

    /** Becomes false when the reader went away. */
    val isOpen: Boolean get() = open

    init {
        writer.isDaemon = true
        writer.start()
    }

    override fun onPacket(data: ByteArray, offset: Int, length: Int, timeMicros: Long) {
        if (!open) return
        if (queuedBytes.get() > maxQueuedBytes) {
            dropped.incrementAndGet()
            return
        }
        val record = Pcap.record(data, offset, length, timeMicros)
        queuedBytes.addAndGet(record.size.toLong())
        queue.add(record)
    }

    private fun writeLoop() {
        try {
            out.write(Pcap.fileHeader())
            while (open) {
                val record = queue.poll(200, TimeUnit.MILLISECONDS) ?: run {
                    out.flush()
                    null
                } ?: continue
                queuedBytes.addAndGet(-record.size.toLong())
                out.write(record)
            }
        } catch (e: IOException) {
            // The reader disconnected.
        } finally {
            open = false
            queue.clear()
            runCatching { out.close() }
            onClosed(this)
        }
    }

    override fun close() {
        open = false
        writer.join(1000)
    }
}
