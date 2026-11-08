package fr.mawena.jarvis

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.util.Log
import fr.mawena.jarvis.ffi.EarEngine

/**
 * Voix de Jarvis : `AudioTrack` en flux, lu sur le fil « jarvis-voix ».
 *
 * [play] et [stop] ne bloquent pas (appelés par l'hôte depuis un fil Rust). La fin de
 * lecture n'est signalée au moteur ([EarEngine.playbackIdle]) que pour la génération du
 * dernier son, quand la tête de lecture a vraiment atteint la fin écrite.
 */
class AudioOut(ctx: Context, private val engine: () -> EarEngine?) {
    private val audioManager = ctx.applicationContext.getSystemService(AudioManager::class.java)
    private val queue = PlaybackQueue()
    private val attributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_ASSISTANT)
        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
        .build()
    private val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
        .setAudioAttributes(attributes)
        .build()

    private val lock = Any()
    private var thread: Thread? = null
    @Volatile private var track: AudioTrack? = null
    private var hasFocus = false
    @Volatile private var released = false

    /** Met un son en file ; le fil de lecture est créé au premier son. */
    fun play(pcm: ByteArray, rate: Int, generation: Long) {
        synchronized(lock) {
            if (released) return
            if (thread == null) {
                thread = Thread(::loop, "jarvis-voix").also { it.start() }
            }
        }
        queue.put(pcm, rate, generation)
    }

    /** `stop_playback` : file vidée, sortie en pause puis vidée, focus rendu. */
    fun stop() {
        queue.stop()
        try {
            track?.let {
                it.pause()
                it.flush()
            }
        } catch (e: IllegalStateException) {
            Log.w(TAG, "arrêt de la lecture", e)
        }
        abandonFocus()
    }

    /** Fin du service : ferme la file, attend brièvement le fil, libère la sortie. */
    fun release() {
        released = true
        queue.close()
        track?.let {
            try {
                it.pause()
                it.flush()
            } catch (_: IllegalStateException) {
            }
        }
        val t = synchronized(lock) { thread }
        t?.interrupt()
        try {
            t?.join(1000)
        } catch (_: InterruptedException) {
        }
        closeTrack()
        abandonFocus()
    }

    private fun loop() {
        var written = 0L // images écrites sur la sortie actuelle
        var trackEpoch = -1L
        while (!released) {
            val chunk = queue.take(200) ?: continue
            val epoch = chunk.epoch
            // Un arrêt est passé entre la prise du son et maintenant : son périmé.
            if (epoch != queue.currentEpoch) continue
            try {
                // Après un arrêt, la tête de lecture n'est plus fiable : sortie recréée.
                if (epoch != trackEpoch) {
                    closeTrack()
                    written = 0
                    trackEpoch = epoch
                }
                if (queue.needsNewOutput(chunk.rate)) {
                    // Fréquence nouvelle : finir le son en cours avant de recréer la sortie.
                    if (track != null) waitHead(written, epoch, stopIfPending = false)
                    closeTrack()
                    written = 0
                    if (!openTrack(chunk.rate)) {
                        queue.outputReleased()
                        continue
                    }
                }
                val t = track ?: continue
                requestFocus()
                if (t.playState != AudioTrack.PLAYSTATE_PLAYING) t.play()
                written += write(t, chunk.pcm, epoch)
                if (queue.hasPending()) continue
                // Plus rien en file : fin de lecture quand la tête atteint la fin écrite.
                if (waitHead(written, epoch, stopIfPending = true)) {
                    queue.drained()?.let { engine()?.playbackIdle(it.toULong()) }
                    abandonFocus()
                }
            } catch (e: Exception) {
                // Sortie fermée sous nos pieds (release() du fil d'arrêt) ou erreur du système :
                // jamais d'exception hors du fil, la sortie sera recréée au prochain son.
                Log.w(TAG, "lecture de la voix interrompue", e)
                closeTrack()
                written = 0
                trackEpoch = -1L
            }
        }
    }

    /** Écrit [pcm] par petits blocs (arrêt rapide) ; renvoie le nombre d'images écrites. */
    private fun write(t: AudioTrack, pcm: ByteArray, epoch: Long): Long {
        var offset = 0
        val block = 4096
        while (offset < pcm.size && !released && queue.currentEpoch == epoch) {
            val n = minOf(block, pcm.size - offset)
            val done = t.write(pcm, offset, n, AudioTrack.WRITE_BLOCKING)
            if (done < 0) {
                Log.w(TAG, "écriture de la voix en erreur : $done")
                break
            }
            offset += done
        }
        return offset / 2L
    }

    /** Attend que la tête de lecture atteigne [frames] ; faux si arrêt demandé entre-temps. */
    private fun waitHead(frames: Long, epoch: Long, stopIfPending: Boolean): Boolean {
        val t = track ?: return false
        val deadline = System.currentTimeMillis() + 30_000
        while (!released && queue.currentEpoch == epoch && System.currentTimeMillis() < deadline) {
            if (stopIfPending && queue.hasPending()) return false
            val head = t.playbackHeadPosition.toLong() and 0xFFFFFFFFL
            if (head >= frames) return true
            try {
                Thread.sleep(10)
            } catch (_: InterruptedException) {
                return false
            }
        }
        return false
    }

    private fun openTrack(rate: Int): Boolean {
        return try {
            val format = AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(rate)
                .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                .build()
            val min = AudioTrack.getMinBufferSize(
                rate,
                AudioFormat.CHANNEL_OUT_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
            )
            val t = AudioTrack.Builder()
                .setAudioAttributes(attributes)
                .setAudioFormat(format)
                .setBufferSizeInBytes(maxOf(min, rate / 5 * 2))
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build()
            track = t
            true
        } catch (e: Exception) {
            Log.w(TAG, "AudioTrack à $rate Hz", e)
            false
        }
    }

    private fun closeTrack() {
        val t = track ?: return
        track = null
        try {
            t.release()
        } catch (_: IllegalStateException) {
        }
        queue.outputReleased()
    }

    private fun requestFocus() {
        synchronized(lock) {
            if (hasFocus) return
            hasFocus = audioManager.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        }
    }

    private fun abandonFocus() {
        synchronized(lock) {
            if (!hasFocus) return
            hasFocus = false
            audioManager.abandonAudioFocusRequest(focusRequest)
        }
    }

    companion object {
        private const val TAG = "JarvisVoix"

        /** Fréquence native de la sortie (Hz), 48 000 si Android ne répond pas. */
        fun nativeRate(): Int =
            AudioTrack.getNativeOutputSampleRate(AudioManager.STREAM_MUSIC).takeIf { it > 0 } ?: 48000
    }
}
