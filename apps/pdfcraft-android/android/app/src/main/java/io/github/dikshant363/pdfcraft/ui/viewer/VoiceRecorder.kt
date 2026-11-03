package io.github.dikshant363.pdfcraft.ui.viewer

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaPlayer
import android.media.MediaRecorder
import android.media.audiofx.NoiseSuppressor
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.RandomAccessFile
import kotlin.math.abs

/** The longest voice comment, and when the recorder warns that the end is near. */
const val VOICE_MAX_MS = 30 * 60 * 1000L
const val VOICE_WARN_MS = 29 * 60 * 1000L

/**
 * Records a voice comment: 16 kHz mono 16-bit PCM from the microphone, with the phone's noise
 * suppression when it has one, into a WAV file. [levels] holds a loudness per 50 ms for the
 * waveform; recording stops by itself at [VOICE_MAX_MS].
 */
class VoiceRecorder(private val file: File, private val scope: CoroutineScope) {
    var recording by mutableStateOf(false)
        private set
    var elapsedMs by mutableLongStateOf(0L)
        private set
    val levels = mutableStateListOf<Float>()
    private var job: Job? = null

    /** Start recording; false when the microphone can't be opened (permission, busy). */
    @SuppressLint("MissingPermission") // the caller asks for RECORD_AUDIO first
    fun start(): Boolean {
        val rate = 16_000
        val minBuf = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (minBuf <= 0) return false
        val rec = try {
            AudioRecord(MediaRecorder.AudioSource.MIC, rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(minBuf, rate / 10 * 2))
        } catch (e: Exception) {
            return false
        }
        if (rec.state != AudioRecord.STATE_INITIALIZED) {
            rec.release()
            return false
        }
        val ns = if (NoiseSuppressor.isAvailable()) runCatching { NoiseSuppressor.create(rec.audioSessionId)?.apply { enabled = true } }.getOrNull() else null
        val out = try {
            RandomAccessFile(file, "rw").apply {
                setLength(0)
                write(ByteArray(44)) // the header, filled in when recording stops
            }
        } catch (e: Exception) {
            ns?.release()
            rec.release()
            return false
        }
        try {
            rec.startRecording()
        } catch (e: Exception) {
            runCatching { out.close() }
            ns?.release()
            rec.release()
            return false
        }
        levels.clear()
        elapsedMs = 0L
        recording = true
        job = scope.launch(Dispatchers.IO) {
            try {
                val buf = ShortArray(rate / 20) // 50 ms
                val bytes = ByteArray(buf.size * 2)
                var samples = 0L
                while (isActive && recording) {
                    val n = rec.read(buf, 0, buf.size)
                    if (n < 0) break
                    if (n == 0) continue
                    var peak = 0
                    for (i in 0 until n) {
                        val s = buf[i].toInt()
                        bytes[2 * i] = (s and 0xFF).toByte()
                        bytes[2 * i + 1] = (s shr 8 and 0xFF).toByte()
                        peak = maxOf(peak, abs(s))
                    }
                    out.write(bytes, 0, n * 2)
                    samples += n
                    val ms = samples * 1000 / rate
                    withContext(Dispatchers.Main) {
                        levels.add(peak / 32768f)
                        elapsedMs = ms
                        if (ms >= VOICE_MAX_MS) recording = false
                    }
                }
                writeWavHeader(out, samples * 2, rate)
            } finally {
                out.close()
                runCatching { rec.stop() }
                ns?.release()
                rec.release()
            }
        }
        return true
    }

    /** Stop recording and wait until the file is complete. */
    suspend fun stop() {
        recording = false
        job?.join()
        job = null
    }

    companion object {
        /** A RIFF/WAVE header for [dataBytes] of 16-bit mono PCM at [rate], at the file's start. */
        fun writeWavHeader(out: RandomAccessFile, dataBytes: Long, rate: Int) {
            val len = dataBytes.coerceAtMost(0xFFFF_FFFFL - 36)
            fun le32(v: Long) = byteArrayOf((v and 0xFF).toByte(), (v shr 8 and 0xFF).toByte(), (v shr 16 and 0xFF).toByte(), (v shr 24 and 0xFF).toByte())
            fun le16(v: Int) = byteArrayOf((v and 0xFF).toByte(), (v shr 8 and 0xFF).toByte())
            out.seek(0)
            out.write("RIFF".toByteArray())
            out.write(le32(len + 36))
            out.write("WAVEfmt ".toByteArray())
            out.write(le32(16))
            out.write(le16(1))
            out.write(le16(1))
            out.write(le32(rate.toLong()))
            out.write(le32(rate * 2L))
            out.write(le16(2))
            out.write(le16(16))
            out.write("data".toByteArray())
            out.write(le32(len))
        }
    }
}

/** Plays a recording, with its position for the progress bar. */
class VoicePlayer(private val scope: CoroutineScope) {
    var playing by mutableStateOf(false)
        private set
    var positionMs by mutableIntStateOf(0)
        private set
    var durationMs by mutableIntStateOf(0)
        private set
    private var player: MediaPlayer? = null
    private var ticker: Job? = null
    private var loaded: File? = null

    /** Get [file] ready to play (reads its duration); false when it can't be played. */
    fun load(file: File): Boolean {
        release()
        val p = MediaPlayer()
        return try {
            p.setDataSource(file.absolutePath)
            p.prepare()
            p.setOnCompletionListener {
                playing = false
                positionMs = 0
                ticker?.cancel()
            }
            player = p
            loaded = file
            durationMs = p.duration.coerceAtLeast(0)
            positionMs = 0
            true
        } catch (e: Exception) {
            p.release()
            false
        }
    }

    fun toggle() {
        val p = player ?: return
        if (playing) {
            p.pause()
            playing = false
            ticker?.cancel()
        } else {
            p.start()
            playing = true
            ticker = scope.launch {
                while (isActive && playing) {
                    positionMs = runCatching { p.currentPosition }.getOrDefault(positionMs)
                    delay(100)
                }
            }
        }
    }

    fun seekTo(fraction: Float) {
        val p = player ?: return
        val ms = (fraction.coerceIn(0f, 1f) * durationMs).toInt()
        runCatching { p.seekTo(ms) }
        positionMs = ms
    }

    fun release() {
        ticker?.cancel()
        player?.release()
        player = null
        loaded = null
        playing = false
        positionMs = 0
        durationMs = 0
    }
}

/** `m:ss` for a duration. */
fun clock(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    return "%d:%02d".format(s / 60, s % 60)
}
