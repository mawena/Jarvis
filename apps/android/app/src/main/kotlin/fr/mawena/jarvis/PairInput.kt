package fr.mawena.jarvis

/** Nettoyage de la saisie d'appairage (pur, sans Android). */
object PairInput {
    private const val PREFIX = "jarvis://pair?"
    private val QUOTES = charArrayOf('"', '\'', '«', '»', '“', '”', '`', '<', '>')

    /**
     * Garde la première ligne non vide, retire espaces et guillemets autour ; `null` si le
     * résultat ne commence pas par `jarvis://pair?` (ou n'a rien après).
     */
    fun clean(raw: String): String? {
        val line = raw.lineSequence().map { it.trim() }.firstOrNull { it.isNotEmpty() } ?: return null
        val uri = line.trim { it.isWhitespace() || it in QUOTES }
        return uri.takeIf { it.startsWith(PREFIX) && it.length > PREFIX.length }
    }
}
