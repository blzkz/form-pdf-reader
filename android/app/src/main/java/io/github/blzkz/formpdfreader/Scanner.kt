package io.github.blzkz.formpdfreader

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.IntentSenderRequest
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Scanning paper documents with the camera, with Google's document scanner
 * (in Google Play services: it finds the edges, crops, straightens and
 * cleans the pages, and gives a PDF). The app itself needs no camera
 * permission; the scan never leaves the device.
 */
object Scanner {
    private val options = GmsDocumentScannerOptions.Builder()
        .setGalleryImportAllowed(true)
        .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_PDF)
        .setScannerMode(GmsDocumentScannerOptions.SCANNER_MODE_FULL)
        .build()

    /** Opens the scanner; the result arrives at [launcher] (see [pdfOf]). */
    fun start(activity: Activity, launcher: ActivityResultLauncher<IntentSenderRequest>) {
        GmsDocumentScanning.getClient(options).getStartScanIntent(activity)
            .addOnSuccessListener { launcher.launch(IntentSenderRequest.Builder(it).build()) }
            .addOnFailureListener { e ->
                Toast.makeText(activity, activity.getString(R.string.scan_unavailable, e.localizedMessage ?: ""), Toast.LENGTH_LONG).show()
            }
    }

    /** The scanned PDF, from the scanner's result (null if cancelled). */
    fun pdfOf(resultCode: Int, data: Intent?): Uri? {
        if (resultCode != Activity.RESULT_OK) return null
        return GmsDocumentScanningResult.fromActivityResultIntent(data)?.pdf?.uri
    }

    /** Copies a scanned PDF to the app's cache as "Scan <date>.pdf" (a new, unsaved document). */
    fun keep(context: Context, uri: Uri): File {
        val stamp = SimpleDateFormat("yyyy-MM-dd HH.mm.ss", Locale.ROOT).format(Date())
        val dir = File(context.cacheDir, "scans").apply { mkdirs() }
        val out = File(dir, context.getString(R.string.scan_name, stamp) + ".pdf")
        requireNotNull(context.contentResolver.openInputStream(uri)) { uri.toString() }.use { i -> out.outputStream().use { i.copyTo(it) } }
        return out
    }
}
