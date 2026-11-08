package fr.mawena.jarvis

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.util.Log
import fr.mawena.jarvis.ffi.EarEngine

/**
 * Micro : `AudioRecord` lu par blocs de 20 ms sur le fil « jarvis-micro ».
 *
 * [start] et [stop] ne bloquent jamais (appelés depuis un fil Rust par l'hôte) : chaque
 * ouverture a sa propre session, son fil, son drapeau d'arrêt ; une session qui s'arrête
 * libère son `AudioRecord` elle-même, et la suivante attend la fin de la précédente
 * avant d'ouvrir le micro.
 */
class AudioIn(private val engine: () -> EarEngine?) {
    private class Session(val previous: Thread?) {
        @Volatile var running = true
        var thread: Thread? = null
    }

    private val lock = Any()
    private var session: Session? = null

    /** Ouvre le micro (sans effet s'il est déjà ouvert). */
    fun start() {
        synchronized(lock) {
            if (session != null) return
            val s = Session(previous = lastThread)
            val t = Thread({ run(s) }, "jarvis-micro")
            s.thread = t
            lastThread = t
            session = s
            t.start()
        }
    }

    /** Ferme le micro ; le fil de lecture se termine seul. */
    fun stop() {
        synchronized(lock) {
            session?.running = false
            session = null
        }
    }

    private var lastThread: Thread? = null

    private fun run(s: Session) {
        try {
            runSession(s)
        } finally {
            // Session finie (y compris après `captureUnavailable`) : un nouveau `start` doit
            // pouvoir rouvrir le micro ; seulement si c'est encore la session courante.
            synchronized(lock) { if (session === s) session = null }
        }
    }

    private fun runSession(s: Session) {
        try {
            s.previous?.join(1000)
        } catch (_: InterruptedException) {
            return
        }
        val (record, rate) = open() ?: run {
            if (s.running) engine()?.captureUnavailable()
            return
        }
        try {
            if (!s.running) return
            record.startRecording()
            if (record.recordingState != AudioRecord.RECORDSTATE_RECORDING) {
                Log.w(TAG, "le micro ne démarre pas")
                if (s.running) engine()?.captureUnavailable()
                return
            }
            engine()?.audioFormat(rate.toUInt(), 1u)
            val buffer = ByteArray(rate / 50 * 2)
            while (s.running) {
                val read = record.read(buffer, 0, buffer.size)
                if (read < 0) {
                    Log.w(TAG, "lecture du micro en erreur : $read")
                    if (s.running) engine()?.captureUnavailable()
                    return
                }
                if (read == 0 || !s.running) continue
                engine()?.pushAudio(if (read == buffer.size) buffer else buffer.copyOf(read))
            }
        } catch (e: Exception) {
            Log.w(TAG, "micro", e)
            if (s.running) engine()?.captureUnavailable()
        } finally {
            try {
                record.stop()
            } catch (_: IllegalStateException) {
            }
            record.release()
        }
    }

    /** `AudioRecord` à 16 kHz, sinon 48 kHz ; `null` si aucun ne s'initialise. */
    private fun open(): Pair<AudioRecord, Int>? {
        for (rate in intArrayOf(16000, 48000)) {
            val min = AudioRecord.getMinBufferSize(
                rate,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
            )
            if (min <= 0) continue
            try {
                val record = AudioRecord(
                    MediaRecorder.AudioSource.VOICE_RECOGNITION,
                    rate,
                    AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                    maxOf(min, 6400),
                )
                if (record.state == AudioRecord.STATE_INITIALIZED) return record to rate
                record.release()
            } catch (e: Exception) {
                Log.w(TAG, "AudioRecord à $rate Hz", e)
            }
        }
        return null
    }

    private companion object {
        const val TAG = "JarvisMicro"
    }
}
