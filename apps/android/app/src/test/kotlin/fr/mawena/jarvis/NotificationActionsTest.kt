package fr.mawena.jarvis

import org.junit.Assert.assertEquals
import org.junit.Test

class NotificationActionsTest {
    @Test
    fun ecouteOuverte() {
        assertEquals(
            listOf(
                R.string.action_mute to ListenService.ACTION_MUTE,
                R.string.action_hard_mute to ListenService.ACTION_HARD_MUTE,
            ),
            EarNotification.actionsFor(muted = false, hardMuted = false),
        )
    }

    @Test
    fun ecouteCoupee() {
        assertEquals(
            listOf(
                R.string.action_unmute to ListenService.ACTION_UNMUTE,
                R.string.action_hard_mute to ListenService.ACTION_HARD_MUTE,
            ),
            EarNotification.actionsFor(muted = true, hardMuted = false),
        )
    }

    @Test
    fun coupureTotaleNeProposeQueRouvrir() {
        val expected = listOf(R.string.action_reopen to ListenService.ACTION_REOPEN)
        assertEquals(expected, EarNotification.actionsFor(muted = false, hardMuted = true))
        assertEquals(expected, EarNotification.actionsFor(muted = true, hardMuted = true))
    }
}
