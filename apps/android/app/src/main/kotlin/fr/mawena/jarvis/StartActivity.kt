package fr.mawena.jarvis

import android.app.Activity
import android.app.NotificationManager

/**
 * Trampoline transparente : démarre l'écoute pendant qu'une activité de l'appli est
 * visible (Android 13 refuse sinon le micro à un service lancé de l'arrière-plan),
 * puis se ferme. Lancée au boot (superposition accordée), par le toucher de la
 * notification, ou après une mise à jour de l'appli.
 */
class StartActivity : Activity() {
    override fun onResume() {
        super.onResume()
        getSystemService(NotificationManager::class.java)?.cancel(EarNotification.ID_START)
        if (intent?.getBooleanExtra(EXTRA_RESTART, false) == true) {
            ListenService.send(this, ListenService.ACTION_RESTART)
        } else {
            ListenService.start(this)
        }
        finish()
    }

    companion object {
        /** Vrai : relance (ACTION_RESTART) au lieu d'un simple démarrage. */
        const val EXTRA_RESTART = "restart"
    }
}
