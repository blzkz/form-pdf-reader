package io.github.blzkz.formpdfreader

import android.app.AlertDialog
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.view.Gravity
import android.view.Menu
import android.view.MenuItem
import android.webkit.MimeTypeMap
import android.widget.FrameLayout
import android.widget.TextView
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.FileProvider
import java.io.File
import uniffi.form_pdf_reader_ffi.DocKind
import uniffi.form_pdf_reader_ffi.InputResult
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * One document per activity. PDF attachments open in a new task, so they
 * show up as separate entries in the recent apps list (like tabs).
 */
class MainActivity : AppCompatActivity() {

    private lateinit var pageView: PdfPageView
    private lateinit var emptyView: TextView

    private var doc: PdfDocument? = null
    private var docUri: Uri? = null
    private var docName = ""
    private var modified = false
        set(value) {
            field = value
            updateTitle()
        }

    /** Pending result of a file picker requested by the form. */
    private var attachmentCallback: ((String?) -> Unit)? = null

    private val openDocument = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) confirmDiscard { open(uri) }
    }

    private val createDocument = registerForActivityResult(ActivityResultContracts.CreateDocument("application/pdf")) { uri ->
        if (uri != null) saveTo(uri, afterSave = null)
    }

    private val pickAttachmentLauncher = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val cb = attachmentCallback
        attachmentCallback = null
        cb?.invoke(uri?.let { copyToCache(it, "picked") }?.absolutePath)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        PdfEngine.start(applicationContext)

        pageView = PdfPageView(this).apply { onInput = ::onFormInput }
        emptyView = TextView(this).apply {
            setText(R.string.empty_hint)
            gravity = Gravity.CENTER
            textSize = 16f
            val pad = (24 * resources.displayMetrics.density).toInt()
            setPadding(pad, pad, pad, pad)
            setOnClickListener { openDocument.launch(arrayOf("application/pdf")) }
        }
        setContentView(FrameLayout(this).apply {
            addView(pageView)
            addView(emptyView)
        })

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                confirmDiscard { finish() }
            }
        })

        intent?.data?.let { open(it) } ?: updateTitle()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.data?.let { uri -> confirmDiscard { open(uri) } }
    }

    // -----------------------------------------------------------------------
    // Menu
    // -----------------------------------------------------------------------

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menuInflater.inflate(R.menu.main, menu)
        return true
    }

    override fun onPrepareOptionsMenu(menu: Menu): Boolean {
        val has = doc != null
        for (id in intArrayOf(R.id.action_save, R.id.action_save_as, R.id.action_attachments, R.id.action_copy_text)) {
            menu.findItem(id)?.isEnabled = has
        }
        return super.onPrepareOptionsMenu(menu)
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        when (item.itemId) {
            R.id.action_open -> openDocument.launch(arrayOf("application/pdf"))
            R.id.action_save -> save()
            R.id.action_save_as -> saveAs()
            R.id.action_attachments -> showAttachments()
            R.id.action_copy_text -> copyPageText()
            R.id.action_about -> AlertDialog.Builder(this)
                .setMessage(getString(R.string.about_text, appVersion()))
                .setPositiveButton(R.string.ok, null)
                .show()
            else -> return super.onOptionsItemSelected(item)
        }
        return true
    }

    // -----------------------------------------------------------------------
    // Opening
    // -----------------------------------------------------------------------

    private fun open(uri: Uri) {
        val name = displayName(uri)
        title = getString(R.string.opening, name)
        val file = try {
            copyToCache(uri, "docs")
        } catch (e: Exception) {
            showError(getString(R.string.open_error, e.message ?: e.toString()))
            return
        }
        val handler = AndroidFormHandler(this)
        PdfEngine.run({
            PdfDocument.open(file.absolutePath, true).also { it.setHandler(handler) }
        }) { r ->
            r.onSuccess { d ->
                // The previous document is freed on the PDFium thread.
                doc?.let { old -> PdfEngine.run({ old.destroy() }) { } }
                doc = d
                docUri = uri
                docName = name
                modified = false
                emptyView.visibility = android.view.View.GONE
                pageView.setDocument(d)
                invalidateOptionsMenu()
                PdfEngine.run({ d.kind() }) { k ->
                    if (k.getOrNull() == DocKind.XFA_DYNAMIC) {
                        Toast.makeText(this, R.string.xfa_info, Toast.LENGTH_LONG).show()
                    }
                }
            }.onFailure { e ->
                updateTitle()
                showError(getString(R.string.open_error, e.message ?: e.toString()))
            }
        }
    }

    /** Copies a content URI into the cache: the core works with files. */
    private fun copyToCache(uri: Uri, dir: String): File {
        val folder = File(cacheDir, dir).apply { mkdirs() }
        val out = File(folder, displayName(uri).replace('/', '_'))
        contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { uri.toString() }
            out.outputStream().use { input.copyTo(it) }
        }
        return out
    }

    private fun displayName(uri: Uri): String {
        if (uri.scheme == "content") {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val name = c.getString(0)
                    if (!name.isNullOrBlank()) return name
                }
            }
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: "document.pdf"
    }

    // -----------------------------------------------------------------------
    // Saving
    // -----------------------------------------------------------------------

    private fun save(afterSave: (() -> Unit)? = null) {
        val uri = docUri
        if (uri == null || !canWrite(uri)) {
            Toast.makeText(this, R.string.save_needs_location, Toast.LENGTH_SHORT).show()
            saveAs()
            return
        }
        saveTo(uri, afterSave)
    }

    private fun saveAs() {
        createDocument.launch(docName.ifBlank { "document.pdf" })
    }

    private fun canWrite(uri: Uri): Boolean =
        uri.scheme == "content" &&
            checkCallingOrSelfUriPermission(uri, Intent.FLAG_GRANT_WRITE_URI_PERMISSION) == android.content.pm.PackageManager.PERMISSION_GRANTED

    private fun saveTo(uri: Uri, afterSave: (() -> Unit)?) {
        val d = doc ?: return
        pageView.commitField {
            PdfEngine.run({ d.saveBytes() }) { r ->
                r.onSuccess { bytes ->
                    try {
                        contentResolver.openOutputStream(uri, "wt").use { out ->
                            requireNotNull(out) { uri.toString() }
                            out.write(bytes)
                        }
                        docUri = uri
                        docName = displayName(uri)
                        modified = false
                        Toast.makeText(this, R.string.saved, Toast.LENGTH_SHORT).show()
                        afterSave?.invoke()
                    } catch (e: Exception) {
                        showError(getString(R.string.save_error, e.message ?: e.toString()))
                    }
                }.onFailure { e -> showError(getString(R.string.save_error, e.message ?: e.toString())) }
            }
        }
    }

    /** Asks before losing unsaved changes. */
    private fun confirmDiscard(then: () -> Unit) {
        if (!modified) return then()
        AlertDialog.Builder(this)
            .setTitle(R.string.unsaved_title)
            .setMessage(R.string.unsaved_message)
            .setPositiveButton(R.string.save) { _, _ -> save(afterSave = then) }
            .setNegativeButton(R.string.discard) { _, _ ->
                modified = false
                then()
            }
            .setNeutralButton(R.string.cancel, null)
            .show()
    }

    // -----------------------------------------------------------------------
    // Form events
    // -----------------------------------------------------------------------

    private fun onFormInput(r: InputResult) {
        if (r.modified && !modified) modified = true
        for (path in r.openFiles) openAttachmentFile(File(path))
        for (u in r.uris) {
            if (u.startsWith("http://") || u.startsWith("https://") || u.startsWith("mailto:")) {
                runCatching { startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(u))) }
            }
        }
        r.submit?.let { url ->
            AlertDialog.Builder(this).setMessage(getString(R.string.submit_unsupported, url)).setPositiveButton(R.string.ok, null).show()
        }
    }

    /** Called by [AndroidFormHandler] when the form asks for a file to attach. */
    fun pickAttachment(callback: (String?) -> Unit) {
        attachmentCallback = callback
        pickAttachmentLauncher.launch(arrayOf("*/*"))
    }

    private fun showAttachments() {
        val d = doc ?: return
        PdfEngine.run({ d.attachments() }) { r ->
            val items = r.getOrNull().orEmpty()
            if (items.isEmpty()) {
                Toast.makeText(this, R.string.no_attachments, Toast.LENGTH_SHORT).show()
                return@run
            }
            val labels = items.map { "${it.fileName}  (${it.size.toLong() / 1024} KB)" }.toTypedArray()
            AlertDialog.Builder(this)
                .setTitle(R.string.attachments)
                .setItems(labels) { _, which ->
                    val a = items[which]
                    PdfEngine.run({ d.attachmentData(a.name) }) { data ->
                        val bytes = data.getOrNull() ?: return@run
                        val folder = File(cacheDir, "attachments").apply { mkdirs() }
                        val f = File(folder, a.fileName.replace('/', '_'))
                        f.writeBytes(bytes)
                        openAttachmentFile(f)
                    }
                }
                .setNegativeButton(R.string.cancel, null)
                .show()
        }
    }

    /** PDFs open in this app (a new task, like a tab); other files in their app. */
    private fun openAttachmentFile(file: File) {
        val uri = FileProvider.getUriForFile(this, "$packageName.files", file)
        val ext = file.extension.lowercase()
        if (ext == "pdf") {
            startActivity(
                Intent(Intent.ACTION_VIEW, uri, this, MainActivity::class.java)
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_DOCUMENT or Intent.FLAG_ACTIVITY_MULTIPLE_TASK),
            )
            return
        }
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
        val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        try {
            startActivity(Intent.createChooser(view, file.name))
        } catch (e: Exception) {
            Toast.makeText(this, R.string.no_app, Toast.LENGTH_SHORT).show()
        }
    }

    private fun copyPageText() {
        val d = doc ?: return
        PdfEngine.run({ (0u until d.pageCount()).joinToString("\n") { d.pageText(it) } }) { r ->
            val text = r.getOrNull().orEmpty().trim()
            if (text.isEmpty()) {
                Toast.makeText(this, R.string.no_text, Toast.LENGTH_SHORT).show()
                return@run
            }
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            cm.setPrimaryClip(ClipData.newPlainText(docName, text))
            Toast.makeText(this, R.string.copied, Toast.LENGTH_SHORT).show()
        }
    }

    // -----------------------------------------------------------------------

    private fun updateTitle() {
        title = when {
            doc == null -> getString(R.string.app_name)
            modified -> "● $docName"
            else -> docName
        }
    }

    private fun showError(message: String) {
        AlertDialog.Builder(this).setMessage(message).setPositiveButton(R.string.ok, null).show()
    }

    private fun appVersion(): String =
        runCatching { packageManager.getPackageInfo(packageName, 0).versionName }.getOrNull() ?: ""

    override fun onDestroy() {
        super.onDestroy()
        val d = doc
        doc = null
        pageView.setDocument(null)
        // Free the document on the PDFium thread.
        if (d != null) PdfEngine.run({ d.destroy() }) { }
    }
}
