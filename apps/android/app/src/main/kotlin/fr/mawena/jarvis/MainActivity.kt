package fr.mawena.jarvis

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast
import fr.mawena.jarvis.ffi.MicView
import fr.mawena.jarvis.ffi.hasWakeword
import fr.mawena.jarvis.ffi.initLogging
import fr.mawena.jarvis.ffi.libraryVersion
import fr.mawena.jarvis.ffi.pairedBrain
import fr.mawena.jarvis.ffi.unpair
import java.util.Locale
import kotlin.concurrent.thread

/**
 * Écran principal : les étapes (autorisations, appairage, mot d'éveil, batterie, démarrage
 * seul), le bouton d'écoute et les réglages. N'ouvre jamais le micro lui-même.
 */
class MainActivity : Activity() {
    private val handler = Handler(Looper.getMainLooper())
    private val refresher = object : Runnable {
        override fun run() {
            refresh()
            handler.postDelayed(this, 1000)
        }
    }

    private lateinit var status: TextView
    private lateinit var listenButton: Button
    private lateinit var nameField: EditText
    private lateinit var autostart: CheckBox
    private lateinit var threshold: SeekBar
    private lateinit var thresholdLabel: TextView

    private val dataDir: String get() = filesDir.absolutePath

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initLogging()
        Log.i("Jarvis", "bibliothèque chargée : ${libraryVersion()}")
        setContentView(R.layout.activity_main)

        status = findViewById(R.id.status)
        listenButton = findViewById(R.id.listen_button)
        nameField = findViewById(R.id.name)
        autostart = findViewById(R.id.autostart)
        threshold = findViewById(R.id.threshold)
        thresholdLabel = findViewById(R.id.threshold_label)

        findViewById<Button>(R.id.permissions_button).setOnClickListener { Permissions.request(this) }
        findViewById<Button>(R.id.pair_button).setOnClickListener {
            startActivity(Intent(this, PairActivity::class.java))
        }
        findViewById<Button>(R.id.wakeword_button).setOnClickListener {
            if (Permissions.micGranted(this)) {
                startActivity(Intent(this, EnrollActivity::class.java))
            } else {
                Permissions.request(this)
            }
        }
        findViewById<Button>(R.id.battery_button).setOnClickListener {
            startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")),
            )
        }
        findViewById<Button>(R.id.overlay_button).setOnClickListener {
            startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")))
        }
        listenButton.setOnClickListener {
            if (ListenService.isRunning()) ListenService.stop(this) else ListenService.start(this)
            handler.postDelayed({ refresh() }, 300)
        }
        findViewById<Button>(R.id.unpair_button).setOnClickListener { confirmUnpair() }

        nameField.setText(Prefs.name(this))
        autostart.isChecked = Prefs.autostart(this)
        autostart.setOnCheckedChangeListener { _, checked -> Prefs.setAutostart(this, checked) }

        threshold.progress = ((Prefs.threshold(this) - Prefs.MIN_THRESHOLD) * 100f).toInt().coerceIn(0, threshold.max)
        showThreshold()
        threshold.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                showThreshold()
                if (fromUser) Prefs.setThreshold(this@MainActivity, Prefs.MIN_THRESHOLD + progress / 100f)
            }

            override fun onStartTrackingTouch(bar: SeekBar?) {}
            override fun onStopTrackingTouch(bar: SeekBar?) {}
        })
    }

    override fun onResume() {
        super.onResume()
        // L'activité est visible : Android rend le micro à un service resté « bloqué ».
        if (ListenService.isRunning() && ListenService.currentView()?.mic == MicView.BLOCKED) {
            ListenService.send(this, ListenService.ACTION_RESTART)
        }
        refresh()
        handler.postDelayed(refresher, 1000)
    }

    override fun onPause() {
        handler.removeCallbacks(refresher)
        saveName()
        super.onPause()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        refresh()
    }

    private fun saveName() {
        val name = nameField.text.toString().trim()
        if (name.isNotEmpty()) Prefs.setName(this, name)
    }

    private fun showThreshold() {
        val value = Prefs.MIN_THRESHOLD + threshold.progress / 100f
        thresholdLabel.text = getString(R.string.threshold_label, String.format(Locale.FRANCE, "%.2f", value))
    }

    private fun confirmUnpair() {
        AlertDialog.Builder(this)
            .setMessage(R.string.unpair_confirm)
            .setPositiveButton(R.string.yes) { _, _ ->
                ListenService.stop(this)
                thread(name = "jarvis-unpair") {
                    val message = try {
                        unpair(dataDir)
                        getString(R.string.unpair_done)
                    } catch (e: Exception) {
                        e.message ?: getString(R.string.unpair_done)
                    }
                    runOnUiThread {
                        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
                        refresh()
                    }
                }
            }
            .setNegativeButton(R.string.no, null)
            .show()
    }

    /** Remet à jour les étapes et le bouton d'écoute. */
    private fun refresh() {
        val permissions = Permissions.allGranted(this)
        step(R.id.permissions_state, R.id.permissions_button, permissions,
            R.string.step_permissions_ok, R.string.step_permissions_missing)

        val brain = pairedBrain(dataDir)
        findViewById<TextView>(R.id.pair_state).text =
            if (brain != null) getString(R.string.step_pair_ok, brain) else getString(R.string.step_pair_none)
        findViewById<Button>(R.id.pair_button).text = getString(R.string.action_pair)

        val wake = hasWakeword(dataDir)
        step(R.id.wakeword_state, R.id.wakeword_button, wake,
            R.string.step_wakeword_ok, R.string.step_wakeword_none, buttonAlways = true)

        val power = getSystemService(PowerManager::class.java)
        step(R.id.battery_state, R.id.battery_button, power.isIgnoringBatteryOptimizations(packageName),
            R.string.step_battery_ok, R.string.step_battery_none)
        step(R.id.overlay_state, R.id.overlay_button, Settings.canDrawOverlays(this),
            R.string.step_overlay_ok, R.string.step_overlay_none)

        val running = ListenService.isRunning()
        val ready = permissions && brain != null && wake
        listenButton.text = getString(if (running) R.string.action_stop else R.string.action_start)
        listenButton.isEnabled = running || ready
        val view = ListenService.currentView()
        status.text = when {
            view != null -> view.text
            running -> getString(R.string.status_starting)
            !ready -> getString(R.string.status_not_ready)
            else -> getString(R.string.status_off)
        }
    }

    private fun step(
        stateId: Int,
        buttonId: Int,
        ok: Boolean,
        okText: Int,
        missingText: Int,
        buttonAlways: Boolean = false,
    ) {
        findViewById<TextView>(stateId).setText(if (ok) okText else missingText)
        findViewById<Button>(buttonId).visibility =
            if (ok && !buttonAlways) android.view.View.GONE else android.view.View.VISIBLE
    }
}
