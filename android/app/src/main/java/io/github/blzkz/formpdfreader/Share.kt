package io.github.blzkz.formpdfreader

import android.app.Activity
import android.content.ClipData
import android.content.Context
import android.content.Intent
import androidx.core.content.FileProvider
import java.io.File

/** Sends a PDF to the app the user chooses (WhatsApp, Telegram, mail…). */
object Share {
    /** A new file in the cache for a copy to share, named like the document. */
    fun target(c: Context, name: String): File {
        val dir = File(c.cacheDir, "share/${System.nanoTime()}").apply { mkdirs() }
        val clean = name.replace('/', '_').ifBlank { "document.pdf" }
        return File(dir, if (clean.endsWith(".pdf", ignoreCase = true)) clean else "$clean.pdf")
    }

    /** Opens the system share sheet with [file], a copy in the cache. */
    fun pdf(activity: Activity, file: File) {
        val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.files", file)
        val send = Intent(Intent.ACTION_SEND)
            .setType("application/pdf")
            .putExtra(Intent.EXTRA_STREAM, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        // The share sheet shows a preview, and the chosen app gets read access.
        send.clipData = ClipData.newRawUri(file.name, uri)
        activity.startActivity(Intent.createChooser(send, activity.getString(R.string.share_title, file.name)))
    }
}
