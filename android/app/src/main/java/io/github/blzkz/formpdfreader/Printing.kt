package io.github.blzkz.formpdfreader

import android.content.Context
import android.os.Bundle
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.print.PageRange
import android.print.PrintAttributes
import android.print.PrintDocumentAdapter
import android.print.PrintDocumentInfo
import android.print.PrintManager
import java.io.File
import java.io.FileOutputStream
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * Printing with the system's print screen: the printers (and "Save as PDF"),
 * a preview, pages, copies, color and two-sided printing.
 *
 * What is printed is the document as it is shown, with the form data and the
 * placed signatures: a flat PDF where every sheet is an image, the same the
 * desktop version sends to the printer. A continuous XFA form is cut into A4
 * sheets.
 */
object Printing {
    /** Resolution of the printed sheets. */
    private const val DPI = 250f

    /** Opens the print screen for [doc]. [onWritten] runs once the sheets are ready. */
    fun print(context: Context, doc: PdfDocument, name: String, onWritten: () -> Unit) {
        val manager = context.getSystemService(Context.PRINT_SERVICE) as PrintManager
        val job = name.removeSuffix(".pdf").removeSuffix(".PDF")
        manager.print(job, Adapter(context.applicationContext, doc, "$job.pdf", onWritten), PrintAttributes.Builder().build())
    }

    private class Adapter(
        private val context: Context,
        private val doc: PdfDocument,
        private val fileName: String,
        private val onWritten: () -> Unit,
    ) : PrintDocumentAdapter() {
        private val file = File(context.cacheDir, "print/${System.nanoTime()}.pdf")
        private var sheets = -1
        /** The flat PDF is made once; the system scales it to the paper. */
        private var written = false

        override fun onLayout(
            oldAttributes: PrintAttributes?,
            newAttributes: PrintAttributes,
            cancellationSignal: CancellationSignal,
            callback: LayoutResultCallback,
            extras: Bundle?,
        ) {
            if (cancellationSignal.isCanceled) return callback.onLayoutCancelled()
            if (sheets >= 0) return callback.onLayoutFinished(info(sheets), false)
            PdfEngine.run({ doc.printSheetCount().toInt() }) { r ->
                when {
                    cancellationSignal.isCanceled -> callback.onLayoutCancelled()
                    r.isSuccess -> {
                        sheets = r.getOrThrow()
                        callback.onLayoutFinished(info(sheets), true)
                    }
                    else -> callback.onLayoutFailed(r.exceptionOrNull()?.message)
                }
            }
        }

        private fun info(pages: Int) = PrintDocumentInfo.Builder(fileName)
            .setContentType(PrintDocumentInfo.CONTENT_TYPE_DOCUMENT)
            .setPageCount(pages)
            .build()

        override fun onWrite(
            pages: Array<out PageRange>,
            destination: ParcelFileDescriptor,
            cancellationSignal: CancellationSignal,
            callback: WriteResultCallback,
        ) {
            PdfEngine.run({
                if (!written) {
                    file.parentFile?.mkdirs()
                    doc.exportPrint(file.path, DPI)
                }
            }) { r ->
                if (r.isFailure) return@run callback.onWriteFailed(r.exceptionOrNull()?.message)
                if (!written) {
                    written = true
                    onWritten()
                }
                if (cancellationSignal.isCanceled) return@run callback.onWriteCancelled()
                // All the sheets: the system keeps the pages chosen.
                Io.run({
                    file.inputStream().use { input ->
                        FileOutputStream(destination.fileDescriptor).use { input.copyTo(it) }
                    }
                }) { w ->
                    if (w.isSuccess) {
                        callback.onWriteFinished(arrayOf(PageRange.ALL_PAGES))
                    } else {
                        callback.onWriteFailed(w.exceptionOrNull()?.message)
                    }
                }
            }
        }

        override fun onFinish() {
            file.delete()
        }
    }
}
