package fr.mawena.jarvis

/** Déroulé de l'enregistrement du mot d'éveil : 8 prises (pur, sans Android). */
class EnrollFlow(val total: Int = TOTAL) {
    /** Numéro de la prise en cours, de 1 à [total]. */
    var take: Int = 1
        private set

    /** Vrai quand les [total] prises sont acceptées. */
    val done: Boolean get() = take > total

    /** La prise est bonne : passer à la suivante. */
    fun accepted() {
        if (!done) take++
    }

    /** Prise trop faible : on recommence sous le même numéro. */
    fun tooQuiet() {
        // Rien à avancer : le numéro ne change pas.
    }

    /** Repart de la première prise (le modèle n'a pas pu être construit). */
    fun restart() {
        take = 1
    }

    companion object {
        const val TOTAL = 8
    }
}
