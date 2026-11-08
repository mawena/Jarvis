package fr.mawena.jarvis

/**
 * État du service d'écoute (pur, testé en JVM) : un seul moteur à la fois, quel que soit
 * le nombre de demandes de démarrage (redémarrage « sticky », toucher de la notification,
 * démarrage automatique, écran principal). Accès depuis un seul fil (celui du moteur).
 */
class ServiceState {
    /** Étape à exécuter, dans l'ordre. */
    enum class Step { STOP_ENGINE, START_ENGINE }

    /** Vrai tant qu'un moteur est lancé. */
    var running = false
        private set

    /** `ACTION_START` : ignoré (liste vide) si le moteur tourne déjà. */
    fun start(): List<Step> {
        if (running) return emptyList()
        running = true
        return listOf(Step.START_ENGINE)
    }

    /** `ACTION_RESTART` : arrêt complet du moteur avant d'en relancer un. */
    fun restart(): List<Step> {
        val steps = if (running) listOf(Step.STOP_ENGINE, Step.START_ENGINE) else listOf(Step.START_ENGINE)
        running = true
        return steps
    }

    /** `ACTION_STOP` / destruction : arrête le moteur s'il tourne. */
    fun stop(): List<Step> {
        if (!running) return emptyList()
        running = false
        return listOf(Step.STOP_ENGINE)
    }

    /** Le démarrage du moteur a échoué : plus rien ne tourne. */
    fun startFailed() {
        running = false
    }
}
