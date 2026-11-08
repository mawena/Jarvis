package fr.mawena.jarvis

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import com.google.zxing.integration.android.IntentIntegrator
import fr.mawena.jarvis.ffi.pairWithUri
import kotlin.concurrent.thread

/** Appairage par QR code ou adresse collée. Rien n'est enregistré tant que l'appairage échoue. */
class PairActivity : Activity() {
    private lateinit var uriField: EditText
    private lateinit var nameField: EditText
    private lateinit var message: TextView
    private lateinit var go: Button
    private lateinit var scan: Button
    private var busy = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_pair)
        title = getString(R.string.pair_title)
        uriField = findViewById(R.id.uri)
        nameField = findViewById(R.id.name)
        message = findViewById(R.id.message)
        go = findViewById(R.id.go)
        scan = findViewById(R.id.scan)
        nameField.setText(Prefs.name(this).ifBlank { Build.MODEL })

        scan.setOnClickListener {
            IntentIntegrator(this)
                .setPrompt(getString(R.string.pair_scan_prompt))
                .setBeepEnabled(false)
                .setOrientationLocked(false)
                .initiateScan()
        }
        go.setOnClickListener { pair(uriField.text.toString()) }
    }

    @Deprecated("Résultat du scanner zxing (API de la bibliothèque).")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        val result = IntentIntegrator.parseActivityResult(requestCode, resultCode, data)
        if (result == null) {
            super.onActivityResult(requestCode, resultCode, data)
            return
        }
        val contents = result.contents ?: return // scan annulé
        uriField.setText(contents)
        pair(contents)
    }

    private fun pair(raw: String) {
        if (busy) return
        val uri = PairInput.clean(raw)
        if (uri == null) {
            message.setText(R.string.pair_invalid)
            return
        }
        val name = nameField.text.toString().trim().ifEmpty { Build.MODEL }
        Prefs.setName(this, name)
        setBusy(true)
        message.setText(R.string.pair_busy)
        val dataDir = filesDir.absolutePath
        thread(name = "jarvis-pair") {
            val outcome = try {
                Result.success(pairWithUri(dataDir, uri, name))
            } catch (e: Exception) {
                Result.failure(e)
            }
            runOnUiThread {
                if (isFinishing || isDestroyed) return@runOnUiThread
                setBusy(false)
                outcome.onSuccess { brain ->
                    message.text = getString(R.string.pair_done, brain)
                    // L'écoute en cours reprend avec le nouvel appairage (l'activité est visible).
                    if (ListenService.isRunning()) ListenService.send(this, ListenService.ACTION_RESTART)
                    finish()
                }.onFailure { e ->
                    message.text = e.message ?: getString(R.string.pair_invalid)
                }
            }
        }
    }

    private fun setBusy(value: Boolean) {
        busy = value
        go.isEnabled = !value
        scan.isEnabled = !value
    }
}
