package fr.mawena.jarvis

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PairInputTest {
    private val uri = "jarvis://pair?host=192.168.1.5&port=7000&token=abc"

    @Test
    fun uriAvecEspacesEtRetourALaLigne() {
        assertEquals(uri, PairInput.clean("\n  \n   $uri  \r\nautre ligne\n"))
    }

    @Test
    fun uriEntreGuillemets() {
        assertEquals(uri, PairInput.clean("\"$uri\""))
        assertEquals(uri, PairInput.clean(" « $uri » "))
    }

    @Test
    fun autreTexteRefuse() {
        assertNull(PairInput.clean("https://exemple.fr"))
        assertNull(PairInput.clean("jarvis://pair?"))
        assertNull(PairInput.clean("   \n  "))
        assertNull(PairInput.clean("bonjour\n$uri"))
    }
}
