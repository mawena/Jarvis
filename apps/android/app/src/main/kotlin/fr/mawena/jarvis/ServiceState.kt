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

    /** Vrai après [close] : plus aucun démarrage (aucun moteur ne doit naître après la destruction). */
    var closed = false
        private set

    /** Destruction du service : arrête le moteur s'il tourne et refuse tout démarrage ensuite. */
    fun close(): List<Step> {
        closed = true
        return stop()
    }

    /** `ACTION_START` : ignoré (liste vide) si le moteur tourne déjà ou si le service est détruit. */
    fun start(): List<Step> {
        if (closed || running) return emptyList()
        running = true
        return listOf(Step.START_ENGINE)
    }

    /** `ACTION_RESTART` : arrêt complet du moteur avant d'en relancer un. */
    fun restart(): List<Step> {
        if (closed) return emptyList()
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

/**
 * Dernier `startId` reçu par `onStartCommand` (fil principal). Une demande d'arrêt ne doit
 * arrêter le service que si aucune demande plus récente (un démarrage tout de suite après)
 * n'est arrivée entre-temps : sinon l'arrêt périmé tuerait le nouveau moteur.
 */
class StartIds {
    @Volatile private var latest = 0

    /** Note l'arrivée d'une commande. */
    fun received(startId: Int) {
        latest = startId
    }

    /** Vrai si [startId] est encore la dernière commande reçue. */
    fun isLatest(startId: Int): Boolean = startId == latest
}
