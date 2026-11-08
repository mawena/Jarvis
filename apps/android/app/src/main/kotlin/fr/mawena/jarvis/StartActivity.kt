package fr.mawena.jarvis

import android.app.Activity

/**
 * Trampoline transparente : démarre l'écoute pendant qu'une activité de l'appli est
 * visible, puis se ferme (tâche 8). Provisoire : se ferme aussitôt.
 */
class StartActivity : Activity() {
    override fun onResume() {
        super.onResume()
        finish()
    }
}
