package fr.mawena.jarvis

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager

/** Autorisations d'exécution de l'oreille : micro et notifications. */
object Permissions {
    const val REQUEST_CODE = 41
    private val NEEDED = arrayOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS)

    fun micGranted(a: Activity): Boolean =
        a.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    fun notificationsGranted(a: Activity): Boolean =
        a.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun allGranted(a: Activity): Boolean = micGranted(a) && notificationsGranted(a)

    /** Demande ce qui manque encore. */
    fun request(a: Activity) {
        val missing = NEEDED.filter { a.checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) a.requestPermissions(missing.toTypedArray(), REQUEST_CODE)
    }
}
