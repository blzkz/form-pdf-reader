package io.github.blzkz.formpdfreader

import android.app.Application
import androidx.appcompat.app.AppCompatDelegate
import java.io.File

class FormPdfApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // Light, dark or following the device (the default).
        AppCompatDelegate.setDefaultNightMode(Prefs.themeMode(this))
        // Loads PDFium in the background so the first document opens sooner.
        PdfEngine.start(this)
        // Copies of documents left behind by a previous run (before any
        // activity makes new ones).
        File(cacheDir, "docs").deleteRecursively()
    }
}
