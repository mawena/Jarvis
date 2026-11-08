package fr.mawena.jarvis

import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import fr.mawena.jarvis.ffi.EarView

/**
 * Service d'écoute au premier plan, type micro (tâche 6). Provisoire : ne fait rien.
 */
class ListenService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        const val ACTION_START = "fr.mawena.jarvis.action.START"
        const val ACTION_STOP = "fr.mawena.jarvis.action.STOP"
        const val ACTION_MUTE = "fr.mawena.jarvis.action.MUTE"
        const val ACTION_UNMUTE = "fr.mawena.jarvis.action.UNMUTE"
        const val ACTION_HARD_MUTE = "fr.mawena.jarvis.action.HARD_MUTE"
        const val ACTION_REOPEN = "fr.mawena.jarvis.action.REOPEN"
        const val ACTION_RESTART = "fr.mawena.jarvis.action.RESTART"

        /** Démarre l'écoute (à appeler pendant qu'une activité est visible). */
        fun start(ctx: Context) {
        }

        /** Arrête l'écoute et retire la notification. */
        fun stop(ctx: Context) {
        }

        /** Envoie une action (`ACTION_*`) au service. */
        fun send(ctx: Context, action: String) {
        }

        /** Vrai si le moteur tourne. */
        fun isRunning(): Boolean = false

        /** Dernier état reçu du moteur, `null` s'il ne tourne pas. */
        fun currentView(): EarView? = null
    }
}
