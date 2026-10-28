package io.github.aabolfazl.perfagent.engine

import io.github.aabolfazl.perfagent.proto.ConnectionState
import io.github.aabolfazl.perfagent.proto.HostSource
import java.io.IOException
import java.net.InetSocketAddress
import java.nio.channels.SelectionKey
import java.nio.channels.SocketChannel
import kotlin.random.Random

/**
 * One app TCP connection, terminated here and continued over a normal socket.
 *
 * Towards the app this plays the server: it answers the SYN once the real server accepted,
 * acknowledges the app's data as it is buffered, and sends the server's data in segments that
 * respect the app's receive window, resending anything the app doesn't acknowledge.
 * Sequence numbers are Ints; differences between them wrap correctly.
 */
internal class TcpFlow(
    private val engine: Engine,
    val key: FlowKey,
    val uid: Int,
    val record: ConnectionRecord,
) : Selectable {
    private enum class State { CONNECTING, ESTABLISHED, CLOSED }

    private var state = State.CONNECTING
    private var channel: SocketChannel? = null
    private var selectionKey: SelectionKey? = null
    private var connectStartedMs = 0L

    // App -> server
    private var rcvNxt = 0
    private val toServer = ByteRing(RECEIVE_WINDOW)
    private var advertisedWindow = 0
    private class PendingSegment(val seq: Int, val data: ByteArray, val fin: Boolean)
    private val outOfOrder = ArrayList<PendingSegment>()
    private var outOfOrderBytes = 0
    private var clientFin = false
    private var serverOutputClosed = false

    // Server -> app. toClient starts at sndUna; the first [sent] bytes were sent and await an ACK.
    private val toClient = ByteRing(SEND_BUFFER)
    private val iss = Random.nextInt()
    private var sndUna = iss + 1
    private var sent = 0
    private var synAcked = false
    private var clientWindow = 0
    private var clientMss = DEFAULT_MSS
    private var serverEof = false
    private var finSent = false
    private var finAcked = false

    private var lastProgressMs = 0L
    private var rto = RTO_MIN_MS
    private var retries = 0
    private var duplicateAcks = 0
    /** Set after a loss until everything sent before it is acknowledged (NewReno recovery). */
    private var inRecovery = false
    private var recoverPoint = 0

    private var sniffBuffer: ByteArray? = null
    private var sniffLength = 0
    private var sniffDone = false

    private val segmentSize: Int get() = minOf(clientMss, engine.segmentSize)

    fun start(syn: TcpSegment) {
        rcvNxt = syn.seq + 1
        clientWindow = syn.window
        if (syn.mss > 0) clientMss = syn.mss
        connectStartedMs = engine.now()

        val ch = try {
            SocketChannel.open()
        } catch (e: IOException) {
            failConnect(e.message)
            return
        }
        channel = ch
        try {
            ch.configureBlocking(false)
            ch.socket().tcpNoDelay = true
            if (!engine.platform.protect(ch.socket())) {
                failConnect("Could not protect socket")
                return
            }
            val target = engine.platform.upstreamAddress(InetSocketAddress(intToIpv4(key.dstIp), key.dstPort))
            if (ch.connect(target)) {
                onConnected()
            } else {
                selectionKey = ch.register(engine.selector, SelectionKey.OP_CONNECT, this)
            }
        } catch (e: IOException) {
            failConnect(e.message)
        }
    }

    override fun onSelected(key: SelectionKey) {
        if (state == State.CONNECTING) {
            if (key.isConnectable) finishConnect()
            return
        }
        if (key.isValid && key.isReadable) readFromServer()
        if (state == State.ESTABLISHED && key.isValid && key.isWritable) writeToServer()
    }

    private fun finishConnect() {
        try {
            channel?.finishConnect()
        } catch (e: IOException) {
            failConnect(e.message)
            return
        }
        onConnected()
    }

    private fun onConnected() {
        state = State.ESTABLISHED
        record.state = ConnectionState.OPEN
        lastProgressMs = engine.now()
        sendSynAck()
        updateInterest()
    }

    private fun sendSynAck() {
        engine.sendTcp(this, iss, rcvNxt, TCP_SYN or TCP_ACK, advertiseWindow(), mss = engine.segmentSize)
    }

    private fun failConnect(reason: String?) {
        engine.sendTcp(this, 0, rcvNxt, TCP_RST or TCP_ACK, 0)
        close(ConnectionState.FAILED, reason ?: "Connection failed")
    }

    /** Handles a segment from the app; [data] holds the whole packet the segment's offsets point into. */
    fun onSegment(segment: TcpSegment, data: ByteArray) {
        if (state == State.CLOSED) return
        if (segment.has(TCP_RST)) {
            channel?.socket()?.let { runCatching { it.setSoLinger(true, 0) } }
            close(ConnectionState.CLOSED, null)
            return
        }
        // A repeated SYN while the server connection is still being set up.
        if (state == State.CONNECTING) return
        if (segment.has(TCP_SYN)) {
            if (!synAcked) sendSynAck()
            return
        }

        if (segment.has(TCP_ACK)) {
            val isPureAck = segment.payloadLength == 0 && !segment.has(TCP_FIN)
            onAck(segment.ack, segment.window, isPureAck)
        }
        // An empty segment from before rcvNxt is a keepalive or a window probe: both need an
        // ACK back, or the app waits (window probe) or drops the connection (keepalive).
        if (segment.payloadLength == 0 && !segment.has(TCP_FIN) && segment.seq - rcvNxt < 0) sendAck()
        if (segment.payloadLength > 0) onData(segment, data)
        if (segment.has(TCP_FIN)) onFin(segment)
        if (state != State.ESTABLISHED) return

        shutdownServerOutputIfDone()
        sendToClient()
        updateInterest()
        closeIfDone()
    }

    private fun onAck(ack: Int, window: Int, isPureAck: Boolean) {
        if (!synAcked) {
            if (ack != iss + 1) return
            synAcked = true
            lastProgressMs = engine.now()
            retries = 0
        }
        val windowGrew = window > clientWindow
        clientWindow = window

        val acked = ack - sndUna
        val outstanding = sent + if (finSent && !finAcked) 1 else 0
        // A growing window is the app reading, not a duplicate ACK.
        if (acked == 0 && outstanding > 0 && isPureAck && !windowGrew) {
            // The app keeps acknowledging the same byte: a segment got lost. Resend it now
            // instead of waiting for the timer (fast retransmit).
            if (++duplicateAcks == 3 && !inRecovery) {
                inRecovery = true
                recoverPoint = sndNxt()
                retransmitFirstUnacked()
            }
            return
        }
        if (acked <= 0 || acked > outstanding) return
        duplicateAcks = 0

        val dataAcked = minOf(acked, sent)
        toClient.skip(dataAcked)
        sent -= dataAcked
        sndUna += dataAcked
        if (acked > dataAcked) {
            finAcked = true
            sndUna += 1
        }
        lastProgressMs = engine.now()
        retries = 0
        rto = RTO_MIN_MS

        if (inRecovery) {
            // A partial ACK means the next segment was lost too.
            if (ack - recoverPoint >= 0) inRecovery = false else retransmitFirstUnacked()
        }
    }

    private fun onData(segment: TcpSegment, data: ByteArray) {
        var offset = segment.payloadOffset
        var length = segment.payloadLength
        val ahead = segment.seq - rcvNxt
        if (clientFin) {
            sendDuplicateAck()
            return
        }
        if (ahead > 0) {
            // A segment before this one was lost. Keep this one if it fits in the window, so that
            // once the gap is resent the ACK can jump past everything received meanwhile.
            val fits = ahead + length <= advertisedWindow && outOfOrderBytes + length <= RECEIVE_WINDOW
            if (fits && outOfOrder.none { it.seq == segment.seq }) {
                outOfOrder.add(PendingSegment(segment.seq, data.copyOfRange(offset, offset + length), segment.has(TCP_FIN)))
                outOfOrderBytes += length
            }
            sendDuplicateAck()
            return
        }
        if (ahead < 0) {
            // Part or all of it is a retransmission of data we already have.
            if (-ahead >= length) {
                sendDuplicateAck()
                return
            }
            offset -= ahead
            length += ahead
        }

        accept(data, offset, length)
        drainOutOfOrder()
        sendAck()
    }

    private fun accept(data: ByteArray, offset: Int, length: Int): Int {
        val accepted = toServer.write(data, offset, length)
        if (accepted > 0) {
            sniff(data, offset, accepted)
            rcvNxt += accepted
        }
        return accepted
    }

    /** Moves kept out-of-order segments that now continue the stream into the buffer. */
    private fun drainOutOfOrder() {
        while (outOfOrder.isNotEmpty() && !clientFin) {
            val next = outOfOrder.minByOrNull { it.seq - rcvNxt }!!
            val start = next.seq - rcvNxt
            if (start > 0) return // still a gap
            outOfOrder.remove(next)
            outOfOrderBytes -= next.data.size

            val skip = -start
            if (skip < next.data.size) {
                val wanted = next.data.size - skip
                // If the buffer is full, the rest is dropped; the app sends it again.
                if (accept(next.data, skip, wanted) < wanted) return
            }
            if (next.fin && next.seq + next.data.size == rcvNxt) {
                rcvNxt += 1
                clientFin = true
            }
        }
        if (clientFin) {
            outOfOrder.clear()
            outOfOrderBytes = 0
        }
    }

    private fun onFin(segment: TcpSegment) {
        if (clientFin) {
            sendAck()
            return
        }
        if (segment.seq + segment.payloadLength != rcvNxt) return
        rcvNxt += 1
        clientFin = true
        sendAck()
    }

    private fun sndNxt(): Int = sndUna + sent + if (finSent && !finAcked) 1 else 0

    private fun sendAck() {
        engine.sendTcp(this, sndNxt(), rcvNxt, TCP_ACK, advertiseWindow())
    }

    /**
     * Repeats the last ACK exactly. The app only counts it as a duplicate ACK (and resends the
     * missing segment right away) if the window is unchanged; otherwise it waits for its
     * retransmission timeout, which keeps doubling.
     */
    private fun sendDuplicateAck() {
        engine.sendTcp(this, sndNxt(), rcvNxt, TCP_ACK, advertisedWindow)
    }

    private fun advertiseWindow(): Int {
        advertisedWindow = minOf(toServer.free, 0xFFFF)
        return advertisedWindow
    }

    private fun sendToClient() {
        if (!synAcked || state != State.ESTABLISHED) return
        while (sent < toClient.size) {
            val windowLeft = clientWindow - sent
            if (windowLeft <= 0) break
            val n = minOf(segmentSize, toClient.size - sent, windowLeft)
            val payload = ByteArray(n)
            toClient.peek(sent, payload, 0, n)
            val flags = if (sent + n == toClient.size) TCP_ACK or TCP_PSH else TCP_ACK
            if (sent == 0) lastProgressMs = engine.now()
            engine.sendTcp(this, sndUna + sent, rcvNxt, flags, advertiseWindow(), payload)
            sent += n
        }
        if (serverEof && !finSent && sent == toClient.size) {
            if (sent == 0) lastProgressMs = engine.now()
            engine.sendTcp(this, sndUna + sent, rcvNxt, TCP_FIN or TCP_ACK, advertiseWindow())
            finSent = true
        }
    }

    private fun readFromServer() {
        val ch = channel ?: return
        val n = try {
            toClient.readFrom(ch)
        } catch (e: IOException) {
            reset(e.message ?: "Server connection failed")
            return
        }
        if (n < 0) serverEof = true
        sendToClient()
        updateInterest()
        closeIfDone()
    }

    private fun writeToServer() {
        val ch = channel ?: return
        try {
            toServer.writeTo(ch)
        } catch (e: IOException) {
            reset(e.message ?: "Server connection failed")
            return
        }
        // Tell the app about the space that just opened up if it was told the buffer was (nearly) full.
        if (advertisedWindow < segmentSize && toServer.free >= segmentSize) sendAck()
        shutdownServerOutputIfDone()
        updateInterest()
        closeIfDone()
    }

    private fun shutdownServerOutputIfDone() {
        if (clientFin && toServer.isEmpty() && !serverOutputClosed) {
            serverOutputClosed = true
            try {
                channel?.shutdownOutput()
            } catch (e: IOException) {
                reset(e.message ?: "Server connection failed")
            }
        }
    }

    private fun updateInterest() {
        if (state != State.ESTABLISHED) return
        val ch = channel ?: return
        var ops = 0
        if (!serverEof && toClient.free > 0) ops = ops or SelectionKey.OP_READ
        if (!toServer.isEmpty()) ops = ops or SelectionKey.OP_WRITE
        val key = selectionKey
        if (key == null || !key.isValid) {
            selectionKey = ch.register(engine.selector, ops, this)
        } else if (key.interestOps() != ops) {
            key.interestOps(ops)
        }
    }

    /** Both sides have finished: the app sent FIN and got all our data and our FIN. */
    private fun closeIfDone() {
        if (state == State.ESTABLISHED && clientFin && serverOutputClosed && finAcked) {
            close(ConnectionState.CLOSED, null)
        }
    }

    fun onTimer(now: Long) {
        when (state) {
            State.CONNECTING -> {
                if (now - connectStartedMs > CONNECT_TIMEOUT_MS) failConnect("Connection timed out")
                return
            }
            State.CLOSED -> return
            State.ESTABLISHED -> Unit
        }

        if (now - record.lastMs > IDLE_TIMEOUT_MS) {
            reset("Idle timeout")
            return
        }

        val waitingForAck = !synAcked || sent > 0 || (finSent && !finAcked)
        if (waitingForAck && now - lastProgressMs >= rto) {
            if (++retries > MAX_RETRIES) {
                reset("App stopped responding")
                return
            }
            rto = minOf(rto * 2, RTO_MAX_MS)
            lastProgressMs = now
            Engine.log.finer { "Timeout on $key: retry $retries, sent=$sent, finSent=$finSent, window=$clientWindow" }
            if (!synAcked) {
                sendSynAck()
                return
            }
            inRecovery = true
            recoverPoint = sndNxt()
            retransmitFirstUnacked()
        } else if (synAcked && clientWindow == 0 && sent < toClient.size && now - lastProgressMs >= rto) {
            // Zero window probe: one byte past the window makes the app report its window again.
            rto = minOf(rto * 2, RTO_MAX_MS)
            lastProgressMs = now
            val probe = ByteArray(1)
            toClient.peek(sent, probe, 0, 1)
            engine.sendTcp(this, sndUna + sent, rcvNxt, TCP_ACK, advertiseWindow(), probe)
        }
    }

    /**
     * Resends only the oldest unacknowledged segment (or our FIN). The app usually kept the
     * segments after the lost one, so its next ACK tells us what else is missing.
     */
    private fun retransmitFirstUnacked() {
        duplicateAcks = 0
        val n = minOf(segmentSize, sent)
        if (n > 0) {
            val payload = ByteArray(n)
            toClient.peek(0, payload, 0, n)
            engine.sendTcp(this, sndUna, rcvNxt, TCP_ACK or TCP_PSH, advertiseWindow(), payload)
        } else if (finSent && !finAcked) {
            engine.sendTcp(this, sndUna, rcvNxt, TCP_FIN or TCP_ACK, advertiseWindow())
        }
    }

    private fun reset(reason: String) {
        if (state == State.CLOSED) return
        Engine.log.fine { "Resetting $key: $reason" }
        engine.sendTcp(this, sndNxt(), rcvNxt, TCP_RST or TCP_ACK, 0)
        channel?.socket()?.let { runCatching { it.setSoLinger(true, 0) } }
        close(ConnectionState.CLOSED, reason)
    }

    override fun toString(): String =
        "TcpFlow($key state=$state synAcked=$synAcked rcvNxt=$rcvNxt toServer=${toServer.size} advertised=$advertisedWindow " +
            "clientFin=$clientFin serverOutputClosed=$serverOutputClosed sndUna=$sndUna sent=$sent toClient=${toClient.size} " +
            "clientWindow=$clientWindow serverEof=$serverEof finSent=$finSent finAcked=$finAcked inRecovery=$inRecovery " +
            "rto=$rto retries=$retries interest=${selectionKey?.takeIf { it.isValid }?.interestOps()})"

    /** Engine shutdown: reset the app's side too so it doesn't wait for a dead connection. */
    fun abort() {
        Engine.log.fine { "Aborting $this" }
        if (state == State.ESTABLISHED) reset("Capture stopped") else close(ConnectionState.CLOSED, "Capture stopped")
    }

    private fun close(finalState: ConnectionState, error: String?) {
        if (state == State.CLOSED) return
        state = State.CLOSED
        selectionKey?.cancel()
        channel?.let { runCatching { it.close() } }
        engine.removeTcp(this)
        engine.tracker.close(record, finalState, error)
    }

    private fun sniff(data: ByteArray, offset: Int, length: Int) {
        if (sniffDone) return
        val buffer = sniffBuffer ?: ByteArray(HostSniffer.MAX_BYTES).also { sniffBuffer = it }
        val n = minOf(length, buffer.size - sniffLength)
        System.arraycopy(data, offset, buffer, sniffLength, n)
        sniffLength += n
        when (val result = HostSniffer.sniff(buffer, sniffLength)) {
            is HostResult.Found -> {
                val source = if (result.source == HostSniffer.SOURCE_TLS) HostSource.TLS_SNI else HostSource.HTTP_HOST
                engine.tracker.setHost(record, result.host, source)
                sniffDone = true
            }
            HostResult.NotFound -> sniffDone = true
            HostResult.NeedMore -> if (sniffLength >= buffer.size) sniffDone = true
        }
        if (sniffDone) sniffBuffer = null
    }

    companion object {
        /** Kept at 64 KiB so the window fits the TCP header without window scaling. */
        const val RECEIVE_WINDOW = 0xFFFF
        const val SEND_BUFFER = 256 * 1024
        const val DEFAULT_MSS = 536
        const val RTO_MIN_MS = 500L
        const val RTO_MAX_MS = 8_000L
        const val MAX_RETRIES = 8
        const val CONNECT_TIMEOUT_MS = 20_000L
        const val IDLE_TIMEOUT_MS = 2 * 60 * 60 * 1000L
    }
}
