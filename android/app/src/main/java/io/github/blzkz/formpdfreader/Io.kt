package io.github.blzkz.formpdfreader

import android.os.Handler
import android.os.Looper
import java.util.concurrent.Executors

/** Background work that does not touch PDFium: file copies, queries, thumbnails. */
object Io {
    private val executor = Executors.newFixedThreadPool(2) { r ->
        Thread(r, "io").apply { isDaemon = true }
    }
    private val main = Handler(Looper.getMainLooper())

    /** Runs [block] in the background and delivers the result on the UI thread. */
    fun <T> run(block: () -> T, onResult: (Result<T>) -> Unit) {
        executor.execute {
            val r = runCatching(block)
            main.post { onResult(r) }
        }
    }
}
