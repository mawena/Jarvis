package fr.mawena.jarvis

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.widget.TextView
import fr.mawena.jarvis.ffi.initLogging
import fr.mawena.jarvis.ffi.libraryVersion

/**
 * Écran principal. Provisoire (tâche 3) : vérifie que la bibliothèque Rust se charge.
 * N'ouvre jamais le micro. Rempli par la tâche 7 (étapes, réglages).
 */
class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initLogging()
        val version = libraryVersion()
        Log.i("Jarvis", "bibliothèque chargée : $version")
        val padding = (16 * resources.displayMetrics.density).toInt()
        setContentView(
            TextView(this).apply {
                text = getString(R.string.library_loaded, version)
                setPadding(padding, padding, padding, padding)
            },
        )
    }
}
