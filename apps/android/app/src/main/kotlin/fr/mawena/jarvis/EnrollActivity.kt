package fr.mawena.jarvis

import android.app.Activity
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Button
import android.widget.TextView
import fr.mawena.jarvis.ffi.EarException
import fr.mawena.jarvis.ffi.enrollFinish
import fr.mawena.jarvis.ffi.enrollReset
import fr.mawena.jarvis.ffi.enrollTake
import kotlin.concurrent.thread

/**
 * Enregistrement du mot d'éveil : 8 prises de « Jarvis » au micro du téléphone.
 * L'écoute est arrêtée pendant l'enregistrement, puis relancée en sortant.
 */
class EnrollActivity : Activity() {
    private val handler = Handler(Looper.getMainLooper())
    private val flow = EnrollFlow()
    private lateinit var prompt: TextView
    private lateinit var take: Button
    private var wasListening = false
    private var busy = false
    private var finished = false

    private val dataDir: String get() = filesDir.absolutePath

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_enroll)
        title = getString(R.string.enroll_title)
        prompt = findViewById(R.id.prompt)
        take = findViewById(R.id.take)

        wasListening = ListenService.isRunning()
        if (wasListening) ListenService.stop(this)

        take.setOnClickListener { if (finished) leave() else startTake() }
        try {
            enrollReset(dataDir)
        } catch (e: Exception) {
            prompt.text = e.message
            take.isEnabled = false
            return
        }
        showTake()
    }

    private fun showTake() {
        take.text = getString(R.string.enroll_button, flow.take, flow.total)
    }

    private fun startTake() {
        if (busy) return
        if (!Permissions.micGranted(this)) {
            prompt.setText(R.string.enroll_need_mic)
            return
        }
        busy = true
        take.isEnabled = false
        prompt.setText(R.string.enroll_prepare)
        handler.postDelayed({
            if (isDestroyed) return@postDelayed
            prompt.setText(R.string.enroll_say)
            val index = flow.take
            thread(name = "jarvis-enroll") {
                val pcm = record()
                val outcome = try {
                    if (pcm == null) {
                        getString(R.string.enroll_mic_failed)
                    } else {
                        enrollTake(dataDir, index.toUInt(), pcm, SAMPLE_RATE.toUInt())
                        null
                    }
                } catch (e: EarException.TooQuiet) {
                    getString(R.string.enroll_too_quiet)
                } catch (e: Exception) {
                    e.message ?: getString(R.string.enroll_mic_failed)
                }
                runOnUiThread { if (!isDestroyed) takeDone(pcm != null && outcome == null, outcome) }
            }
        }, PREPARE_MS)
    }

    private fun takeDone(accepted: Boolean, problem: String?) {
        busy = false
        take.isEnabled = true
        if (!accepted) {
            flow.tooQuiet()
            prompt.text = problem
            showTake()
            return
        }
        flow.accepted()
        if (!flow.done) {
            prompt.text = ""
            showTake()
            return
        }
        // Dernière prise acceptée : construction du modèle.
        busy = true
        take.isEnabled = false
        thread(name = "jarvis-enroll-finish") {
            val outcome = try {
                enrollFinish(dataDir)
                null
            } catch (e: Exception) {
                e.message ?: getString(R.string.enroll_mic_failed)
            }
            runOnUiThread {
                if (isDestroyed) return@runOnUiThread
                busy = false
                take.isEnabled = true
                if (outcome == null) {
                    finished = true
                    prompt.text = getString(R.string.enroll_done, flow.total)
                    take.setText(R.string.enroll_close)
                } else {
                    // Le modèle n'a pas pu être construit : on recommence les prises.
                    prompt.text = outcome
                    try {
                        enrollReset(dataDir)
                    } catch (_: Exception) {
                    }
                    flow.restart()
                    showTake()
                }
            }
        }
    }

    /** Ouvre le micro 1,5 s et le referme aussitôt ; `null` si le micro ne s'ouvre pas. */
    private fun record(): ByteArray? {
        val total = SAMPLE_RATE * 2 * TAKE_MS / 1000
        val minBuffer = AudioRecord.getMinBufferSize(
            SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minBuffer <= 0) return null
        val recorder = try {
            AudioRecord(
                MediaRecorder.AudioSource.VOICE_RECOGNITION, SAMPLE_RATE,
                AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
                maxOf(minBuffer, total),
            )
        } catch (_: Exception) {
            return null
        }
        try {
            if (recorder.state != AudioRecord.STATE_INITIALIZED) return null
            val out = ByteArray(total)
            recorder.startRecording()
            var read = 0
            while (read < total) {
                val n = recorder.read(out, read, total - read)
                if (n <= 0) return null
                read += n
            }
            return out
        } catch (_: Exception) {
            return null
        } finally {
            try {
                recorder.stop()
            } catch (_: Exception) {
            }
            recorder.release()
        }
    }

    /** Sortie : l'écoute reprend si elle tournait (l'activité est encore visible). */
    private fun leave() {
        if (wasListening) {
            wasListening = false
            ListenService.start(this)
        }
        finish()
    }

    @Deprecated("Retour : relance l'écoute avant de quitter.")
    override fun onBackPressed() {
        if (busy) return
        if (wasListening) {
            wasListening = false
            ListenService.start(this)
        }
        super.onBackPressed()
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        super.onDestroy()
    }

    private companion object {
        const val SAMPLE_RATE = 16_000
        const val PREPARE_MS = 800L
        const val TAKE_MS = 1500
    }
}
