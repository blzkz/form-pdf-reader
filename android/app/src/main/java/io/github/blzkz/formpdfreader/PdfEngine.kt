package io.github.blzkz.formpdfreader

import android.content.Context
import android.os.Handler
import android.os.Looper
import java.util.Locale
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.Future
import uniffi.form_pdf_reader_ffi.initPdfium
import uniffi.form_pdf_reader_ffi.setLanguage
import uniffi.form_pdf_reader_ffi.setTempDir

/**
 * Runs every call to the Rust core.
 *
 * PDFium and its V8 engine only work from the thread that initialised them,
 * so all calls go through one dedicated thread. The UI thread never calls the
 * core directly: it submits work here and gets the result back on the main
 * thread. The form's dialogs ([AndroidFormHandler]) block this thread, not
 * the UI, while the user answers.
 */
object PdfEngine {
    private val executor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "pdfium").apply { isDaemon = true }
    }
    private val main = Handler(Looper.getMainLooper())

    @Volatile
    private var started = false

    /** Loads the native libraries and initialises PDFium (once). */
    fun start(context: Context) {
        if (started) return
        started = true
        val tmp = context.cacheDir.resolve("tmp").absolutePath
        // The app's language (Android 13+ lets each app have its own); the
        // core uses English for a language it does not have.
        val lang = context.resources.configuration.locales[0]?.language ?: Locale.getDefault().language
        executor.submit {
            // libform_pdf_reader_ffi.so depends on libpdfium.so.
            System.loadLibrary("pdfium")
            setLanguage(lang)
            setTempDir(tmp)
            initPdfium()
        }
    }

    /** Runs [block] on the PDFium thread. */
    fun <T> submit(block: () -> T): Future<T> = executor.submit(Callable { block() })

    /** Runs [block] on the PDFium thread and delivers the result on the UI thread. */
    fun <T> run(block: () -> T, onResult: (Result<T>) -> Unit) {
        executor.execute {
            val r = runCatching(block)
            main.post { onResult(r) }
        }
    }

    /** True when called from the PDFium thread. */
    fun onEngineThread(): Boolean = Thread.currentThread().name == "pdfium"
}
