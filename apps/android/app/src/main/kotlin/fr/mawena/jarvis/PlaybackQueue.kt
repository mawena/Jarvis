package fr.mawena.jarvis

/**
 * File de lecture de la voix de Jarvis (pure : ni Android ni UniFFI, testée en JVM).
 *
 * Elle garde l'ordre des sons, retient la génération du dernier son reçu et dit quand
 * la fin de lecture doit être signalée au moteur : seulement quand la file est vide et
 * que le dernier son est fini (un ancien son ne remet jamais l'oreille au repos).
 * Toutes les méthodes sont sûres entre fils.
 */
class PlaybackQueue {
    /**
     * Un son : octets PCM 16 bits little-endian, fréquence en Hz, numéro de génération, et
     * époque (voir [currentEpoch]) au moment où il est entré : si elle n'est plus la
     * courante, un [stop] est passé entre-temps et le son est périmé.
     */
    class Chunk(val pcm: ByteArray, val rate: Int, val generation: Long, val epoch: Long)

    private val lock = Object()
    private val chunks = ArrayDeque<Chunk>()
    private var lastGeneration = -1L
    private var reportedGeneration = -1L
    private var outputRate = 0
    private var epoch = 0L
    private var closed = false

    /** Compteur incrémenté par [stop] : un lecteur le compare pour abandonner un son périmé. */
    val currentEpoch: Long get() = synchronized(lock) { epoch }

    /** Ajoute un son en fin de file ; ne bloque jamais. */
    fun put(pcm: ByteArray, rate: Int, generation: Long) {
        synchronized(lock) {
            if (closed) return
            chunks.addLast(Chunk(pcm, rate, generation, epoch))
            lastGeneration = generation
            lock.notifyAll()
        }
    }

    /** Vrai s'il reste au moins un son en file. */
    fun hasPending(): Boolean = synchronized(lock) { chunks.isNotEmpty() }

    /** Prend le prochain son sans attendre, `null` si la file est vide. */
    fun poll(): Chunk? = synchronized(lock) { chunks.removeFirstOrNull() }

    /** Attend au plus [timeoutMs] un son ; `null` si rien n'est arrivé ou si la file est fermée. */
    fun take(timeoutMs: Long): Chunk? {
        val deadline = System.currentTimeMillis() + timeoutMs
        synchronized(lock) {
            while (chunks.isEmpty() && !closed) {
                val left = deadline - System.currentTimeMillis()
                if (left <= 0) return null
                lock.wait(left)
            }
            return chunks.removeFirstOrNull()
        }
    }

    /**
     * Le lecteur a fini d'écrire et la tête de lecture a atteint la fin : renvoie la
     * génération à signaler au moteur, ou `null` s'il reste des sons en file, s'il n'y a
     * rien eu à jouer, ou si cette fin a déjà été signalée.
     */
    fun drained(): Long? = synchronized(lock) {
        if (chunks.isNotEmpty() || lastGeneration < 0 || reportedGeneration == lastGeneration) {
            null
        } else {
            reportedGeneration = lastGeneration
            lastGeneration
        }
    }

    /**
     * `stop_playback` : vide la file et oublie ce qui restait à signaler (le moteur s'est
     * déjà remis au repos, pas d'appel à `playbackIdle`).
     */
    fun stop() {
        synchronized(lock) {
            chunks.clear()
            reportedGeneration = lastGeneration
            epoch++
            lock.notifyAll()
        }
    }

    /** Ferme la file (fin du service) : plus rien n'entre, les attentes se terminent. */
    fun close() {
        synchronized(lock) {
            closed = true
            chunks.clear()
            epoch++
            lock.notifyAll()
        }
    }

    /**
     * Vrai si la sortie audio doit être recréée pour jouer à [rate] (première lecture ou
     * changement de fréquence) ; mémorise alors cette fréquence.
     */
    fun needsNewOutput(rate: Int): Boolean = synchronized(lock) {
        if (rate == outputRate) {
            false
        } else {
            outputRate = rate
            true
        }
    }

    /** La sortie audio a été libérée : la prochaine lecture la recréera. */
    fun outputReleased() {
        synchronized(lock) { outputRate = 0 }
    }
}
