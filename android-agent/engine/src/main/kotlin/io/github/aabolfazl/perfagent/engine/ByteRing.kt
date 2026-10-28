package io.github.aabolfazl.perfagent.engine

import java.nio.ByteBuffer
import java.nio.channels.ReadableByteChannel
import java.nio.channels.WritableByteChannel

/** Fixed-size FIFO byte buffer that reads from and writes to channels without extra copies. */
internal class ByteRing(val capacity: Int) {
    private val buffer = ByteArray(capacity)
    private var head = 0

    var size = 0
        private set

    val free: Int get() = capacity - size

    fun isEmpty() = size == 0

    fun write(src: ByteArray, offset: Int, length: Int): Int {
        val n = minOf(length, free)
        var copied = 0
        while (copied < n) {
            val tail = (head + size) % capacity
            val chunk = minOf(n - copied, capacity - tail)
            System.arraycopy(src, offset + copied, buffer, tail, chunk)
            size += chunk
            copied += chunk
        }
        return n
    }

    /** Reads from [channel] into free space. Returns bytes read, or -1 at end of stream. */
    fun readFrom(channel: ReadableByteChannel): Int {
        var total = 0
        while (free > 0) {
            val tail = (head + size) % capacity
            val chunk = minOf(free, capacity - tail)
            val n = channel.read(ByteBuffer.wrap(buffer, tail, chunk))
            if (n < 0) return if (total > 0) total else -1
            if (n == 0) break
            size += n
            total += n
            if (n < chunk) break
        }
        return total
    }

    /** Writes buffered bytes to [channel] and drops what was written. */
    fun writeTo(channel: WritableByteChannel): Int {
        var total = 0
        while (size > 0) {
            val chunk = minOf(size, capacity - head)
            val n = channel.write(ByteBuffer.wrap(buffer, head, chunk))
            if (n <= 0) break
            skip(n)
            total += n
            if (n < chunk) break
        }
        return total
    }

    /** Copies [length] bytes starting [offset] bytes after the head, without consuming them. */
    fun peek(offset: Int, dst: ByteArray, dstOffset: Int, length: Int) {
        require(offset >= 0 && length >= 0 && offset + length <= size)
        var copied = 0
        while (copied < length) {
            val pos = (head + offset + copied) % capacity
            val chunk = minOf(length - copied, capacity - pos)
            System.arraycopy(buffer, pos, dst, dstOffset + copied, chunk)
            copied += chunk
        }
    }

    fun skip(n: Int) {
        require(n in 0..size)
        head = (head + n) % capacity
        size -= n
        if (size == 0) head = 0
    }
}
