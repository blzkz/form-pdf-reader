package io.github.blzkz.formpdfreader

import android.content.Context
import android.util.Log
import uniffi.form_pdf_reader_ffi.OcrEngine

/**
 * The OCR models (PaddleOCR's text detection and Latin recognition), loaded
 * from the app's assets the first time they are needed.
 */
object OcrModels {
    @Volatile
    private var engine: OcrEngine? = null

    /** The OCR engine, or null if the models cannot be loaded. */
    fun engine(c: Context): OcrEngine? =
        engine ?: synchronized(this) {
            engine ?: runCatching {
                val a = c.applicationContext.assets
                OcrEngine.load(
                    a.open("models/det.onnx").use { it.readBytes() },
                    a.open("models/rec.onnx").use { it.readBytes() },
                    a.open("models/rec_dict.txt").use { it.readBytes() }.toString(Charsets.UTF_8),
                )
            }.onFailure { Log.w("OcrModels", "OCR models", it) }.getOrNull().also { engine = it }
        }
}
