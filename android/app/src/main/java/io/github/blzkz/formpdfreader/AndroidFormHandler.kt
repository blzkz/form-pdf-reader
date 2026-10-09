package io.github.blzkz.formpdfreader

import android.os.Handler
import android.os.Looper
import android.widget.EditText
import android.widget.FrameLayout
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicReference
import uniffi.form_pdf_reader_ffi.AlertAnswer
import uniffi.form_pdf_reader_ffi.AlertButtons
import uniffi.form_pdf_reader_ffi.FormHandler

/**
 * Dialogs requested by the form's scripts (app.alert, app.response and the
 * attachment file picker).
 *
 * The core calls these methods on the PDFium thread and waits for the
 * answer, as Adobe Reader does. Each method shows the dialog on the UI thread
 * and blocks the PDFium thread until the user answers.
 */
class AndroidFormHandler(private val activity: MainActivity) : FormHandler {
    private val main = Handler(Looper.getMainLooper())

    private fun <T> waitForUi(default: T, show: (done: (T) -> Unit) -> Unit): T {
        val result = AtomicReference(default)
        val latch = CountDownLatch(1)
        main.post {
            if (activity.isFinishing || activity.isDestroyed) {
                latch.countDown()
                return@post
            }
            var finished = false
            show { value ->
                if (!finished) {
                    finished = true
                    result.set(value)
                    latch.countDown()
                }
            }
        }
        latch.await()
        return result.get()
    }

    override fun alert(title: String, message: String, buttons: AlertButtons): AlertAnswer =
        waitForUi(AlertAnswer.OK) { done ->
            val b = MaterialAlertDialogBuilder(activity)
                .setTitle(if (title.isBlank() || title == "Alert") activity.getString(R.string.form) else title)
                .setMessage(message)
                .setCancelable(false)
            when (buttons) {
                AlertButtons.OK -> b.setPositiveButton(R.string.ok) { _, _ -> done(AlertAnswer.OK) }
                AlertButtons.OK_CANCEL -> {
                    b.setPositiveButton(R.string.ok) { _, _ -> done(AlertAnswer.OK) }
                    b.setNegativeButton(R.string.cancel) { _, _ -> done(AlertAnswer.CANCEL) }
                }
                AlertButtons.YES_NO -> {
                    b.setPositiveButton(R.string.yes) { _, _ -> done(AlertAnswer.YES) }
                    b.setNegativeButton(R.string.no) { _, _ -> done(AlertAnswer.NO) }
                }
                AlertButtons.YES_NO_CANCEL -> {
                    b.setPositiveButton(R.string.yes) { _, _ -> done(AlertAnswer.YES) }
                    b.setNegativeButton(R.string.no) { _, _ -> done(AlertAnswer.NO) }
                    b.setNeutralButton(R.string.cancel) { _, _ -> done(AlertAnswer.CANCEL) }
                }
            }
            b.show()
        }

    override fun pickFile(): String? = waitForUi<String?>(null) { done ->
        activity.pickAttachment { path -> done(path) }
    }

    override fun ask(question: String, title: String, defaultValue: String): String? =
        waitForUi<String?>(null) { done ->
            val input = EditText(activity).apply { setText(defaultValue) }
            val pad = (20 * activity.resources.displayMetrics.density).toInt()
            val box = FrameLayout(activity).apply {
                setPadding(pad, 0, pad, 0)
                addView(input)
            }
            MaterialAlertDialogBuilder(activity)
                .setTitle(title.ifBlank { activity.getString(R.string.form) })
                .setMessage(question)
                .setView(box)
                .setCancelable(false)
                .setPositiveButton(R.string.ok) { _, _ -> done(input.text.toString()) }
                .setNegativeButton(R.string.cancel) { _, _ -> done(null) }
                .show()
        }
}
