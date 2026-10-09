package io.github.blzkz.formpdfreader

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.webkit.MimeTypeMap
import android.widget.FrameLayout
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.io.File
import java.io.FileInputStream
import uniffi.form_pdf_reader_ffi.DocKind
import uniffi.form_pdf_reader_ffi.InputResult
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * The viewer: one document per activity. It opens a file on the device
 * ([EXTRA_PATH]) or a content URI (the system picker, other apps). PDF
 * attachments open in a new task, so they show up as separate entries in
 * the recent apps list (like tabs).
 */
class MainActivity : AppCompatActivity() {

    private lateinit var toolbar: MaterialToolbar
    private lateinit var progress: View
    private lateinit var pageView: PdfPageView

    private var doc: PdfDocument? = null
    /** What is shown: where it is saved back to. */
    private var current: DocItem? = null
    /** The copy in the cache that PDFium reads. */
    private var docFile: File? = null
    private var modified = false
        set(value) {
            field = value
            updateTitle()
        }

    /** Pending result of a file picker requested by the form. */
    private var attachmentCallback: ((String?) -> Unit)? = null

    private val openDocument = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) confirmDiscard { open(itemFor(uri)) }
    }

    private val createDocument = registerForActivityResult(ActivityResultContracts.CreateDocument("application/pdf")) { uri ->
        if (uri != null) saveTo(itemFor(uri), afterSave = null)
    }

    private val pickAttachmentLauncher = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val cb = attachmentCallback
        attachmentCallback = null
        val path = uri?.let { runCatching { copyToCache(itemFor(it), "picked") }.getOrNull() }?.absolutePath
        cb?.invoke(path)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        PdfEngine.start(applicationContext)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        setContentView(R.layout.activity_viewer)

        toolbar = findViewById(R.id.toolbar)
        progress = findViewById(R.id.progress)
        setSupportActionBar(toolbar)
        toolbar.setNavigationOnClickListener { onBackPressedDispatcher.onBackPressed() }
        applySystemInsets(findViewById(R.id.root))

        pageView = PdfPageView(this).apply { onInput = ::onFormInput }
        findViewById<FrameLayout>(R.id.page_container).addView(pageView)

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                confirmDiscard { finish() }
            }
        })

        val item = itemFor(intent)
        if (item == null) {
            finish()
            return
        }
        open(item)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        itemFor(intent)?.let { item -> confirmDiscard { open(item) } }
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
            else -> return super.onOptionsItemSelected(item)
        }
        return true
    }

    // -----------------------------------------------------------------------
    // Opening
    // -----------------------------------------------------------------------

    private fun itemFor(intent: Intent): DocItem? {
        intent.getStringExtra(EXTRA_PATH)?.let { return DocItem(File(it).name, path = it) }
        val uri = intent.data ?: return null
        return itemFor(uri)
    }

    private fun itemFor(uri: Uri): DocItem {
        if (uri.scheme == "file") {
            val path = uri.path.orEmpty()
            return DocItem(File(path).name, path = path)
        }
        return DocItem(displayName(uri), uri = uri.toString())
    }

    private fun open(item: DocItem) {
        title = getString(R.string.opening, item.name)
        progress.visibility = View.VISIBLE
        val continuous = Prefs.continuousXfa(this)
        val handler = AndroidFormHandler(this)
        // The core works with files: the document is copied to the cache.
        Io.run({ copyToCache(item, "docs/${System.nanoTime()}") }) { copied ->
            val file = copied.getOrElse { e -> return@run openFailed(item, e) }
            PdfEngine.run({
                PdfDocument.open(file.absolutePath, continuous).also { it.setHandler(handler) }
            }) { r ->
                progress.visibility = View.GONE
                r.onSuccess { d ->
                    // The previous document is freed on the PDFium thread.
                    release()
                    doc = d
                    docFile = file
                    current = item
                    modified = false
                    pageView.setDocument(d)
                    invalidateOptionsMenu()
                    remember(item, file.length())
                    if (continuous) {
                        PdfEngine.run({ d.kind() }) { k ->
                            if (k.getOrNull() == DocKind.XFA_DYNAMIC) {
                                Toast.makeText(this, R.string.xfa_info, Toast.LENGTH_LONG).show()
                            }
                        }
                    }
                }.onFailure { e ->
                    file.parentFile?.deleteRecursively()
                    openFailed(item, e)
                }
            }
        }
    }

    private fun openFailed(item: DocItem, e: Throwable) {
        progress.visibility = View.GONE
        updateTitle()
        // A recent document that is gone or no longer accessible.
        if (e is SecurityException || e is java.io.FileNotFoundException) Recents.remove(this, item.key)
        MaterialAlertDialogBuilder(this)
            .setMessage(getString(R.string.open_error, e.message ?: e.toString()))
            .setPositiveButton(R.string.ok, null)
            .setOnDismissListener { if (doc == null) finish() }
            .show()
    }

    /** Adds the document to the start screen's recents. */
    private fun remember(item: DocItem, size: Long) {
        val uri = item.contentUri
        if (uri != null) {
            // Attachments extracted to the cache are not worth remembering.
            if (uri.authority == "$packageName.files") return
            // Keeps access to documents chosen with the system picker.
            val rw = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            runCatching { contentResolver.takePersistableUriPermission(uri, rw) }
                .recoverCatching { contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        }
        Recents.add(this, item.copy(size = size, time = System.currentTimeMillis()))
    }

    /** Copies a document into the cache: the core works with files. */
    private fun copyToCache(item: DocItem, dir: String): File {
        val folder = File(cacheDir, dir).apply { mkdirs() }
        val out = File(folder, item.name.replace('/', '_'))
        val input = if (item.path != null) FileInputStream(item.path) else contentResolver.openInputStream(requireNotNull(item.contentUri))
        requireNotNull(input) { item.key }.use { i -> out.outputStream().use { i.copyTo(it) } }
        return out
    }

    private fun displayName(uri: Uri): String {
        if (uri.scheme == "content") {
            runCatching {
                contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                    if (c.moveToFirst()) {
                        val name = c.getString(0)
                        if (!name.isNullOrBlank()) return name
                    }
                }
            }
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: "document.pdf"
    }

    // -----------------------------------------------------------------------
    // Saving
    // -----------------------------------------------------------------------

    private fun save(afterSave: (() -> Unit)? = null) {
        val item = current ?: return
        val uri = item.contentUri
        if (item.path == null && (uri == null || !canWrite(uri))) {
            Toast.makeText(this, R.string.save_needs_location, Toast.LENGTH_SHORT).show()
            saveAs()
            return
        }
        saveTo(item, afterSave)
    }

    private fun saveAs() {
        createDocument.launch(current?.name ?: "document.pdf")
    }

    private fun canWrite(uri: Uri): Boolean =
        uri.scheme == "content" &&
            checkCallingOrSelfUriPermission(uri, Intent.FLAG_GRANT_WRITE_URI_PERMISSION) == PackageManager.PERMISSION_GRANTED

    private fun saveTo(target: DocItem, afterSave: (() -> Unit)?) {
        val d = doc ?: return
        pageView.commitField {
            PdfEngine.run({ d.saveBytes() }) { r ->
                r.onSuccess { bytes ->
                    try {
                        if (target.path != null) {
                            File(target.path).writeBytes(bytes)
                        } else {
                            contentResolver.openOutputStream(requireNotNull(target.contentUri), "wt").use { out ->
                                requireNotNull(out) { target.key }
                                out.write(bytes)
                            }
                        }
                        current = target
                        modified = false
                        Thumbnails.forget(target.key)
                        remember(target, bytes.size.toLong())
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
        MaterialAlertDialogBuilder(this)
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
        r.submit?.let { url -> showError(getString(R.string.submit_unsupported, url)) }
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
            MaterialAlertDialogBuilder(this)
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
            cm.setPrimaryClip(ClipData.newPlainText(current?.name, text))
            Toast.makeText(this, R.string.copied, Toast.LENGTH_SHORT).show()
        }
    }

    // -----------------------------------------------------------------------

    private fun updateTitle() {
        val name = current?.name
        title = when {
            name == null -> getString(R.string.app_name)
            modified -> "● $name"
            else -> name
        }
    }

    private fun showError(message: String) {
        MaterialAlertDialogBuilder(this).setMessage(message).setPositiveButton(R.string.ok, null).show()
    }

    /** Frees the document and its copy on the PDFium thread. */
    private fun release() {
        val d = doc ?: return
        val file = docFile
        doc = null
        docFile = null
        PdfEngine.run({ d.destroy() }) { file?.parentFile?.deleteRecursively() }
    }

    override fun onDestroy() {
        super.onDestroy()
        if (::pageView.isInitialized) pageView.setDocument(null)
        release()
    }

    companion object {
        /** Path of a file on the device to open. */
        const val EXTRA_PATH = "io.github.blzkz.formpdfreader.PATH"

        fun intentFor(c: Context, item: DocItem): Intent =
            if (item.path != null) {
                Intent(c, MainActivity::class.java).putExtra(EXTRA_PATH, item.path)
            } else {
                intentFor(c, requireNotNull(item.contentUri))
            }

        fun intentFor(c: Context, uri: Uri): Intent = Intent(Intent.ACTION_VIEW, uri, c, MainActivity::class.java)
    }
}
