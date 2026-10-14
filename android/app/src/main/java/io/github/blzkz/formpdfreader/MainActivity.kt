package io.github.blzkz.formpdfreader

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.security.KeyChain
import android.text.InputType
import android.text.format.DateUtils
import android.view.KeyEvent
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.webkit.MimeTypeMap
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.PopupMenu
import android.widget.TextView
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import androidx.core.view.isVisible
import androidx.core.widget.doAfterTextChanged
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.button.MaterialButton
import com.google.android.material.button.MaterialButtonToggleGroup
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.tabs.TabLayout
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.IOException
import java.util.UUID
import uniffi.form_pdf_reader_ffi.DigitalPlacement
import uniffi.form_pdf_reader_ffi.P12Certificate
import uniffi.form_pdf_reader_ffi.certificateIsRsa
import uniffi.form_pdf_reader_ffi.DocKind
import uniffi.form_pdf_reader_ffi.InputResult
import uniffi.form_pdf_reader_ffi.PdfDocument
import uniffi.form_pdf_reader_ffi.PdfException

/**
 * The viewer: the open documents as tabs, like the desktop version. It opens
 * a file on the device ([EXTRA_PATH]) or a content URI (the system picker,
 * other apps); documents opened while it is showing (another PDF from
 * another app, a PDF attachment) are added as new tabs.
 *
 * On tablets the toolbar also has the view, zoom and page controls, and a
 * hardware keyboard has the desktop shortcuts.
 */
class MainActivity : AppCompatActivity() {

    /** An open document: one tab. */
    private class DocTab(val view: PdfPageView, var item: DocItem) {
        var doc: PdfDocument? = null
        /** The copy in the cache that PDFium reads. */
        var file: File? = null
        var modified = false
        var loading = true
        var kind: DocKind? = null
        /** Where its unsaved changes are kept on the device (see [Drafts]). */
        var draftId: String = UUID.randomUUID().toString()
        /** A draft to open instead of the document (changes recovered). */
        var recoverFrom: File? = null
        /** Changes made so far, and how many of them the draft has. */
        var edits = 0
        var draftedEdits = 0
        /** The password the PDF asked for, while it is open (never stored). */
        var password: String? = null
    }

    private lateinit var toolbar: MaterialToolbar
    private lateinit var tabs: TabLayout
    private lateinit var progress: View
    private lateinit var container: FrameLayout
    private lateinit var pageChip: TextView
    private lateinit var tools: View
    private lateinit var layoutGroup: MaterialButtonToggleGroup
    private lateinit var zoomLevel: MaterialButton
    private lateinit var pageLabel: MaterialButton
    private lateinit var pagePrev: View
    private lateinit var pageNext: View
    private lateinit var searchBar: View
    private lateinit var searchInput: EditText
    private lateinit var searchCount: TextView

    private val docs = mutableListOf<DocTab>()
    private var current: DocTab? = null
    private var isTablet = false
    private val formHandler by lazy { AndroidFormHandler(this) }
    private val main = Handler(Looper.getMainLooper())
    private val hidePageChip = Runnable { pageChip.visibility = View.GONE }

    /** Pending result of a file picker requested by the form. */
    private var attachmentCallback: ((String?) -> Unit)? = null

    /** The tab being saved with "Save as". */
    private var saveAsTab: DocTab? = null

    /** Digital signature waiting for a certificate file or the signed copy's place. */
    private var certificateCallback: ((Uri?) -> Unit)? = null
    private var signedCallback: ((Uri?) -> Unit)? = null

    private val pickCertificate = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val cb = certificateCallback
        certificateCallback = null
        cb?.invoke(uri)
    }

    private val createSigned = registerForActivityResult(ActivityResultContracts.CreateDocument("application/pdf")) { uri ->
        val cb = signedCallback
        signedCallback = null
        cb?.invoke(uri)
    }

    /** A recent document that has to be chosen again: replaced by the one chosen. */
    private var replacing: DocItem? = null

    private val openDocument = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        val old = replacing
        replacing = null
        if (uri == null) {
            if (docs.isEmpty()) finish()
            return@registerForActivityResult
        }
        old?.let { Recents.remove(this, it.key) }
        open(itemFor(uri))
    }

    private val createDocument = registerForActivityResult(ActivityResultContracts.CreateDocument("application/pdf")) { uri ->
        val t = saveAsTab
        saveAsTab = null
        if (uri != null && t != null && t in docs) saveTo(t, itemFor(uri), afterSave = null)
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

        val root = findViewById<ViewGroup>(R.id.root)
        toolbar = findViewById(R.id.toolbar)
        tabs = findViewById(R.id.tabs)
        progress = findViewById(R.id.progress)
        container = findViewById(R.id.page_container)
        pageChip = findViewById(R.id.page_chip)
        tools = findViewById(R.id.tools)
        layoutGroup = findViewById(R.id.layout_group)
        zoomLevel = findViewById(R.id.zoom_level)
        pageLabel = findViewById(R.id.page_label)
        pagePrev = findViewById(R.id.page_prev)
        pageNext = findViewById(R.id.page_next)
        searchBar = findViewById(R.id.search_bar)
        searchInput = findViewById(R.id.search_input)
        searchCount = findViewById(R.id.search_count)

        setSupportActionBar(toolbar)
        toolbar.setNavigationOnClickListener { onBackPressedDispatcher.onBackPressed() }
        applySystemInsets(root)

        isTablet = resources.configuration.smallestScreenWidthDp >= 600
        if (isTablet) {
            // As on the desktop: the tabs on top, the tools below, no title.
            supportActionBar?.setDisplayShowTitleEnabled(false)
            tools.visibility = View.VISIBLE
            root.removeView(tabs)
            root.addView(tabs, 0)
        }
        setupTools()
        setupSearch()

        tabs.addOnTabSelectedListener(object : TabLayout.OnTabSelectedListener {
            override fun onTabSelected(tab: TabLayout.Tab) {
                (tab.tag as? DocTab)?.let { select(it) }
            }

            override fun onTabUnselected(tab: TabLayout.Tab) {}

            override fun onTabReselected(tab: TabLayout.Tab) {}
        })

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (searchBar.isVisible) closeSearch() else confirmAll { finish() }
            }
        })

        // Changes left unsaved when the app was closed: the system closed it in
        // the background (and now opens it again), or the user chose to
        // recover them on the start screen.
        if (savedInstanceState != null || intent.getBooleanExtra(EXTRA_RECOVER, false)) recoverDrafts()
        val item = itemFor(intent)
        if (item == null) {
            if (docs.isEmpty()) finish()
            return
        }
        openOrRecover(item)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.getBooleanExtra(EXTRA_RECOVER, false)) recoverDrafts()
        itemFor(intent)?.let { openOrRecover(it) }
    }

    override fun onStop() {
        super.onStop()
        // Android may close the app while it is in the background: the
        // changes are kept on the device until they are saved or discarded.
        for (t in docs) if (t.modified && t.edits != t.draftedEdits) writeDraft(t)
    }

    // -----------------------------------------------------------------------
    // Tabs
    // -----------------------------------------------------------------------

    /**
     * Opens [item] in a new tab, or shows its tab if it is already open. With
     * [draft], the tab shows the changes kept in it, still to be saved.
     */
    private fun open(item: DocItem, draft: Drafts.Draft? = null) {
        docs.firstOrNull { it.item.key == item.key }?.let {
            selectTab(it)
            return
        }
        val view = PdfPageView(this).apply { visibility = View.GONE }
        val t = DocTab(view, item)
        if (draft != null) {
            t.draftId = draft.id
            t.recoverFrom = draft.file
            t.modified = true
            t.edits = 1
            t.draftedEdits = 1
        }
        Drafts.open += t.draftId
        view.onInput = { r -> onFormInput(t, r) }
        view.onSignaturesChanged = { markModified(t) }
        view.onSignatureError = { e -> showError(getString(R.string.sign_error, e.message ?: "")) }
        view.onStateChanged = {
            if (t === current) {
                updateTools()
                showPageChip()
            }
        }
        // Below the page number chip.
        container.addView(view, container.childCount - 1)
        docs += t
        val tab = tabs.newTab().setCustomView(R.layout.item_tab).setTag(t)
        tab.customView?.findViewById<View>(R.id.tab_close)?.setOnClickListener { closeTab(t) }
        tabs.addTab(tab, true)
        updateTabLabel(t)
        updateTabsVisibility()
        load(t)
    }

    private fun select(t: DocTab) {
        val old = current
        if (old != null && old !== t) {
            old.view.visibility = View.GONE
            old.view.clearSelection()
            old.view.clearSearch()
            old.view.trimMemory()
        }
        current = t
        t.view.visibility = View.VISIBLE
        t.view.requestFocus()
        progress.isVisible = t.loading
        updateTitle()
        updateTools()
        invalidateOptionsMenu()
        // The search goes on in the tab now shown.
        if (searchBar.isVisible && old !== t) search(searchInput.text.toString())
    }

    private fun selectTab(t: DocTab) {
        tabs.getTabAt(docs.indexOf(t))?.select()
    }

    private fun switchTab(delta: Int) {
        if (docs.size < 2) return
        val i = (docs.indexOf(current) + delta).mod(docs.size)
        selectTab(docs[i])
    }

    /** Closes a tab (asking about unsaved changes); the last one closes the viewer. */
    private fun closeTab(t: DocTab) {
        confirmDiscard(t) {
            removeTab(t)
            if (docs.isEmpty()) finish()
        }
    }

    private fun removeTab(t: DocTab) {
        val i = docs.indexOf(t)
        if (i < 0) return
        docs.removeAt(i)
        if (current === t) current = null
        // Selects the next tab.
        tabs.getTabAt(i)?.let { tabs.removeTab(it) }
        container.removeView(t.view)
        t.view.setDocument(null)
        release(t)
        Drafts.open -= t.draftId
        // Saved or discarded: its changes are no longer kept.
        if (!t.modified) deleteDraft(t)
        updateTabsVisibility()
        if (docs.isEmpty()) progress.visibility = View.GONE
    }

    private fun updateTabLabel(t: DocTab) {
        val title = tabs.getTabAt(docs.indexOf(t))?.customView?.findViewById<TextView>(R.id.tab_title) ?: return
        title.text = if (t.modified) "● ${t.item.name}" else t.item.name
    }

    private fun updateTabsVisibility() {
        tabs.isVisible = isTablet || docs.size > 1
    }

    /** Frees the document and its copy on the PDFium thread. */
    private fun release(t: DocTab) {
        val d = t.doc
        val file = t.file
        t.doc = null
        t.file = null
        if (d != null) {
            PdfEngine.run({ d.destroy() }) { file?.parentFile?.deleteRecursively() }
        } else {
            file?.parentFile?.deleteRecursively()
        }
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

    private fun load(t: DocTab) {
        val item = t.item
        val draft = t.recoverFrom
        val continuous = Prefs.continuousXfa(this)
        // The core works with files: the document is copied to the cache.
        Io.run({ copyToCache(item, "docs/${System.nanoTime()}", from = draft) }) { copied ->
            val file = copied.getOrElse { e -> return@run openFailed(t, e) }
            PdfEngine.run({
                PdfDocument.open(file.absolutePath, continuous, t.password).also { it.setHandler(formHandler) }
            }) { r ->
                r.onSuccess { d ->
                    if (t !in docs) {
                        // Closed while it was opening.
                        PdfEngine.run({ d.destroy() }) { file.parentFile?.deleteRecursively() }
                        return@run
                    }
                    t.doc = d
                    t.file = file
                    t.loading = false
                    t.view.setDocument(d)
                    remember(item, if (draft == null) file.length() else item.size)
                    if (!isTemporary(item)) Thumbnails.capture(this, item.key, d)
                    if (draft != null) Toast.makeText(this, R.string.draft_recovered, Toast.LENGTH_SHORT).show()
                    if (t === current) {
                        progress.visibility = View.GONE
                        updateTitle()
                        invalidateOptionsMenu()
                    }
                    PdfEngine.run({ d.kind() }) { k ->
                        t.kind = k.getOrNull()
                        // (After recovering changes, that notice is the one to read.)
                        if (continuous && t.kind == DocKind.XFA_DYNAMIC && draft == null) {
                            Toast.makeText(this, R.string.xfa_info, Toast.LENGTH_LONG).show()
                        }
                    }
                }.onFailure { e ->
                    file.parentFile?.deleteRecursively()
                    when {
                        t !in docs -> Unit
                        e is PdfException.Password -> askPassword(t, wrong = false)
                        e is PdfException.WrongPassword -> askPassword(t, wrong = true)
                        else -> openFailed(t, e)
                    }
                }
            }
        }
    }

    /** The PDF needs a password to open: asks for it and opens it again (Cancel closes its tab). */
    private fun askPassword(t: DocTab, wrong: Boolean) {
        val pad = (20 * resources.displayMetrics.density).toInt()
        val field = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            setSingleLine()
            imeOptions = EditorInfo.IME_ACTION_DONE
            contentDescription = getString(R.string.password_title)
        }
        val box = FrameLayout(this).apply {
            setPadding(pad, pad / 2, pad, 0)
            addView(field)
        }
        val message = getString(R.string.password_prompt, t.item.name) +
            if (wrong) "\n\n" + getString(R.string.password_wrong) else ""
        val dialog = MaterialAlertDialogBuilder(this)
            .setTitle(R.string.password_title)
            .setMessage(message)
            .setView(box)
            .setPositiveButton(R.string.open) { _, _ ->
                t.password = field.text.toString()
                load(t)
            }
            .setNegativeButton(R.string.cancel) { _, _ ->
                removeTab(t)
                finishIfIdle()
            }
            .setCancelable(false)
            .create()
        field.setOnEditorActionListener { _, action, _ ->
            if (action == EditorInfo.IME_ACTION_DONE) {
                dialog.getButton(android.app.AlertDialog.BUTTON_POSITIVE).performClick()
                true
            } else {
                false
            }
        }
        dialog.window?.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
        dialog.show()
        field.requestFocus()
    }

    /** Explains why a document could not be opened, in the user's terms. */
    private fun openFailed(t: DocTab, e: Throwable) {
        val item = t.item
        removeTab(t)
        val noAccess = e is SecurityException
        val missing = !noAccess && e is FileNotFoundException
        val b = MaterialAlertDialogBuilder(this).setTitle(R.string.error_open_title)
        when {
            // The permission given by another app has expired (or was revoked).
            noAccess -> {
                Recents.markNeedsPick(this, item.key)
                b.setMessage(getString(R.string.error_no_access, item.name))
                    .setPositiveButton(R.string.choose_file) { _, _ -> chooseAgain(item) }
                    .setNegativeButton(R.string.cancel, null)
            }
            missing -> {
                Recents.remove(this, item.key)
                b.setMessage(getString(R.string.error_not_found, item.name))
                    .setPositiveButton(R.string.choose_file) { _, _ -> chooseAgain(item) }
                    .setNegativeButton(R.string.cancel, null)
            }
            // The core explains it already (password, damaged file…), translated.
            e is PdfException -> b.setMessage(getString(R.string.open_error, e.message.orEmpty()))
                .setPositiveButton(R.string.ok, null)
            else -> b.setMessage(getString(R.string.error_open_generic, item.name))
                .setPositiveButton(R.string.ok, null)
                .setNeutralButton(R.string.details) { _, _ -> showDetails(e) }
        }
        b.setOnDismissListener { finishIfIdle() }
        b.show()
    }

    private var showingDetails = false

    /** Closes the viewer when no document is left and nothing else is showing. */
    private fun finishIfIdle() {
        if (docs.isEmpty() && replacing == null && !showingDetails) finish()
    }

    /** Opens the system picker to choose [item] again; it replaces it in the recents. */
    private fun chooseAgain(item: DocItem) {
        replacing = item
        openDocument.launch(arrayOf("application/pdf"))
    }

    private fun showDetails(e: Throwable) {
        showingDetails = true
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.details)
            .setMessage(e.message ?: e.toString())
            .setPositiveButton(R.string.ok, null)
            .setOnDismissListener {
                showingDetails = false
                finishIfIdle()
            }
            .show()
    }

    /** Copies of attachments and documents shared by this app itself. */
    private fun isTemporary(item: DocItem): Boolean =
        item.path?.startsWith(cacheDir.absolutePath) == true || item.contentUri?.authority == "$packageName.files"

    /** Adds the document to the start screen's recents. */
    private fun remember(item: DocItem, size: Long) {
        if (isTemporary(item)) return
        item.contentUri?.let { uri ->
            // Keeps access to documents chosen with the system picker.
            val rw = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            runCatching { contentResolver.takePersistableUriPermission(uri, rw) }
                .recoverCatching { contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        }
        // Without a lasting permission (documents shared by another app for that
        // time only), it will have to be chosen again to open it from Recents.
        val lasting = item.contentUri?.let { uri ->
            contentResolver.persistedUriPermissions.any { it.uri == uri && it.isReadPermission }
        } ?: true
        Recents.add(this, item.copy(size = size, time = System.currentTimeMillis(), needsPick = !lasting))
    }

    /** Copies a document (or [from], its draft) into the cache: the core works with files. */
    private fun copyToCache(item: DocItem, dir: String, from: File? = null): File {
        val folder = File(cacheDir, dir).apply { mkdirs() }
        val out = File(folder, item.name.replace('/', '_'))
        val input = when {
            from != null -> FileInputStream(from)
            item.path != null -> FileInputStream(item.path)
            else -> contentResolver.openInputStream(requireNotNull(item.contentUri))
        }
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

    private fun save(tab: DocTab? = current, afterSave: (() -> Unit)? = null) {
        val t = tab ?: return
        if (t.doc == null) return
        val item = t.item
        val uri = item.contentUri
        val writable = !isTemporary(item) && (item.path != null || (uri != null && canWrite(uri)))
        if (!writable) {
            Toast.makeText(this, R.string.save_needs_location, Toast.LENGTH_SHORT).show()
            saveAs(t)
            return
        }
        saveTo(t, item, afterSave)
    }

    private fun saveAs(tab: DocTab? = current) {
        val t = tab ?: return
        if (t.doc == null) return
        saveAsTab = t
        createDocument.launch(t.item.name)
    }

    private fun canWrite(uri: Uri): Boolean =
        uri.scheme == "content" &&
            checkCallingOrSelfUriPermission(uri, Intent.FLAG_GRANT_WRITE_URI_PERMISSION) == PackageManager.PERMISSION_GRANTED

    private fun saveTo(t: DocTab, target: DocItem, afterSave: (() -> Unit)?) {
        val d = t.doc ?: return
        t.view.commitField {
            PdfEngine.run({ d.saveBytes() }) { r ->
                val bytes = r.getOrElse { e -> return@run saveFailed(t, e) }
                val edits = t.edits
                Io.run({ writeDocument(target, bytes) }) { w ->
                    w.onSuccess {
                        t.view.refreshSignatures()
                        t.item = target
                        // Changes made while it was being written are still unsaved.
                        t.modified = t.edits != edits
                        if (!t.modified) deleteDraft(t)
                        remember(target, bytes.size.toLong())
                        // The thumbnail in Recents, with the data saved.
                        if (!isTemporary(target)) t.doc?.let { Thumbnails.capture(this, target.key, it) }
                        updateTabLabel(t)
                        if (t === current) updateTitle()
                        Toast.makeText(this, R.string.saved, Toast.LENGTH_SHORT).show()
                        afterSave?.invoke()
                    }.onFailure { e -> saveFailed(t, e) }
                }
            }
        }
    }

    /**
     * Writes a saved document over [target]. A file on the device is replaced
     * in one go. A document of another app (content URI) can only be
     * overwritten: if writing fails half way, what it had before is written
     * back, so it is never left empty or cut. Runs in the background.
     */
    private fun writeDocument(target: DocItem, bytes: ByteArray) {
        target.path?.let { path ->
            val f = File(path)
            val tmp = File(f.parentFile, ".${f.name}.${System.nanoTime()}.tmp")
            val replaced = runCatching {
                tmp.outputStream().use {
                    it.write(bytes)
                    it.fd.sync()
                }
                tmp.renameTo(f)
            }.getOrDefault(false)
            tmp.delete()
            // A folder where no other file can be created: written in place.
            if (!replaced) f.writeBytes(bytes)
            return
        }
        val uri = requireNotNull(target.contentUri)
        val before = runCatching { contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
        var overwritten = false
        try {
            val out = requireNotNull(contentResolver.openOutputStream(uri, "wt")) { target.key }
            overwritten = true
            out.use { it.write(bytes) }
            // Some apps do not report errors while writing: check the size
            // (-1 when the app does not know it).
            val size = contentResolver.openFileDescriptor(uri, "r")?.use { it.statSize } ?: -1L
            if (size >= 0 && size != bytes.size.toLong()) throw IOException(getString(R.string.save_incomplete))
        } catch (e: Exception) {
            if (overwritten && before != null && before.isNotEmpty()) {
                runCatching { contentResolver.openOutputStream(uri, "wt")?.use { it.write(before) } }
            }
            throw e
        }
    }

    // -----------------------------------------------------------------------
    // Unsaved changes kept on the device
    // -----------------------------------------------------------------------

    /** Keeps the changes of [t] in its draft (see [Drafts]). */
    private fun writeDraft(t: DocTab) {
        val d = t.doc ?: return
        val edits = t.edits
        t.view.commitField {
            PdfEngine.run({ d.saveBytes() }) { r ->
                val bytes = r.getOrNull() ?: return@run
                if (t !in docs || !t.modified) return@run
                val id = t.draftId
                val item = t.item
                // Guardar escribe en el PDF las firmas puestas.
                t.view.refreshSignatures()
                Io.run({ Drafts.write(this, id, item, bytes) }) { w ->
                    if (w.isSuccess) t.draftedEdits = edits
                }
            }
        }
    }

    private fun deleteDraft(t: DocTab) {
        val id = t.draftId
        t.draftedEdits = t.edits
        Io.run({ Drafts.delete(this, id) }) {}
    }

    /** Opens every document with changes kept from a previous run. */
    private fun recoverDrafts() {
        for (draft in Drafts.list(this, notOpen = true)) open(draft.item, draft)
    }

    /** Opens [item]; if it has changes kept from a previous run, asks whether to recover them. */
    private fun openOrRecover(item: DocItem) {
        val draft = Drafts.find(this, item.key)
        if (draft == null || docs.any { it.item.key == item.key }) return open(item)
        val ago = DateUtils.getRelativeTimeSpanString(draft.time, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.drafts_title)
            .setMessage(getString(R.string.draft_found, item.name, ago))
            .setPositiveButton(R.string.recover) { _, _ -> open(item, draft) }
            .setNegativeButton(R.string.discard) { _, _ ->
                Drafts.delete(this, draft.id)
                open(item)
            }
            .setCancelable(false)
            .show()
    }

    /** Asks before losing the unsaved changes of [t]. */
    private fun confirmDiscard(t: DocTab, then: () -> Unit) {
        if (!t.modified) return then()
        selectTab(t)
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.unsaved_title)
            .setMessage(getString(R.string.unsaved_named, t.item.name))
            .setPositiveButton(R.string.save) { _, _ -> save(t, afterSave = then) }
            .setNegativeButton(R.string.discard) { _, _ ->
                t.modified = false
                deleteDraft(t)
                updateTabLabel(t)
                then()
            }
            .setNeutralButton(R.string.cancel, null)
            .show()
    }

    /** Asks about every tab with unsaved changes, one after the other. */
    private fun confirmAll(then: () -> Unit) {
        val t = docs.firstOrNull { it.modified } ?: return then()
        confirmDiscard(t) { confirmAll(then) }
    }

    // -----------------------------------------------------------------------
    // Menu, tools and shortcuts
    // -----------------------------------------------------------------------

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menuInflater.inflate(R.menu.main, menu)
        return true
    }

    override fun onPrepareOptionsMenu(menu: Menu): Boolean {
        val has = current?.doc != null
        for (id in intArrayOf(R.id.action_search, R.id.action_share, R.id.action_save, R.id.action_sign, R.id.action_digital, R.id.action_save_as, R.id.action_print, R.id.action_attachments, R.id.action_copy_text, R.id.action_go_to_page, R.id.action_view)) {
            menu.findItem(id)?.isEnabled = has
        }
        // Tablets have these in the toolbar.
        menu.findItem(R.id.action_view)?.isVisible = !isTablet
        menu.findItem(R.id.action_go_to_page)?.isVisible = !isTablet
        val checked = when (current?.view?.layoutMode) {
            PdfPageView.Layout.SINGLE -> R.id.view_single
            PdfPageView.Layout.TWO_PAGES -> R.id.view_two
            else -> R.id.view_continuous
        }
        menu.findItem(checked)?.isChecked = true
        if (isTablet) fitActionIcons(menu)
        return super.onPrepareOptionsMenu(menu)
    }

    /**
     * Tablets: as many action icons as fit beside the centred tools; the rest
     * go to the overflow menu (in portrait, Open and Attachments). Otherwise
     * the toolbar would push the tools aside and the zoom would leave the
     * middle of the screen.
     */
    private fun fitActionIcons(menu: Menu) {
        val density = resources.displayMetrics.density
        val toolsDp = if (tools.width > 0) tools.width / density else 600f
        // Room on the right half, minus the overflow button (48 dp each).
        val room = resources.configuration.screenWidthDp / 2f - toolsDp / 2f - 16f
        val slots = (room / 48f).toInt() - 1
        for ((i, id) in intArrayOf(R.id.action_search, R.id.action_save, R.id.action_sign, R.id.action_share, R.id.action_attachments, R.id.action_open).withIndex()) {
            menu.findItem(id)?.setShowAsAction(
                if (i < slots) MenuItem.SHOW_AS_ACTION_ALWAYS else MenuItem.SHOW_AS_ACTION_NEVER,
            )
        }
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        // Rotated: the action icons that fit beside the tools change.
        invalidateOptionsMenu()
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        when (item.itemId) {
            R.id.action_search -> openSearch()
            R.id.action_share -> share()
            R.id.action_open -> openDocument.launch(arrayOf("application/pdf"))
            R.id.action_save -> save()
            R.id.action_sign -> sign()
            R.id.action_digital -> showDigitalSignatures()
            R.id.action_save_as -> saveAs()
            R.id.action_print -> print()
            R.id.action_attachments -> showAttachments()
            R.id.action_copy_text -> copyPageText()
            R.id.action_go_to_page -> askPage()
            R.id.action_close_tab -> current?.let { closeTab(it) }
            R.id.view_continuous -> setLayout(PdfPageView.Layout.CONTINUOUS)
            R.id.view_single -> setLayout(PdfPageView.Layout.SINGLE)
            R.id.view_two -> setLayout(PdfPageView.Layout.TWO_PAGES)
            else -> return super.onOptionsItemSelected(item)
        }
        return true
    }

    private var updatingTools = false

    private fun setupTools() {
        layoutGroup.addOnButtonCheckedListener { _, id, checked ->
            if (!checked || updatingTools) return@addOnButtonCheckedListener
            setLayout(
                when (id) {
                    R.id.layout_single -> PdfPageView.Layout.SINGLE
                    R.id.layout_two -> PdfPageView.Layout.TWO_PAGES
                    else -> PdfPageView.Layout.CONTINUOUS
                },
            )
        }
        findViewById<View>(R.id.zoom_out).setOnClickListener { current?.view?.zoomOut() }
        findViewById<View>(R.id.zoom_in).setOnClickListener { current?.view?.zoomIn() }
        zoomLevel.setOnClickListener { showZoomMenu(it) }
        pagePrev.setOnClickListener { current?.view?.previousPage() }
        pageNext.setOnClickListener { current?.view?.nextPage() }
        pageLabel.setOnClickListener { askPage() }

        // The view buttons (left) and the page controls (right) take the same
        // width, so the zoom stays in the middle of the screen, between the
        // two pages of the two-page view. The page label changes width.
        val left = findViewById<View>(R.id.tools_left)
        val right = findViewById<View>(R.id.tools_right)
        tools.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ ->
            val width = maxOf(naturalWidth(left), naturalWidth(right))
            if (left.minimumWidth != width || right.minimumWidth != width) {
                left.minimumWidth = width
                right.minimumWidth = width
                tools.post {
                    tools.requestLayout()
                    invalidateOptionsMenu()
                }
            }
        }
    }

    /** Width of [v] for its content, without its minimum width. */
    private fun naturalWidth(v: View): Int {
        val min = v.minimumWidth
        v.minimumWidth = 0
        val unspecified = View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED)
        v.measure(unspecified, unspecified)
        v.minimumWidth = min
        return v.measuredWidth
    }

    private fun setLayout(mode: PdfPageView.Layout) {
        current?.view?.setLayout(mode)
        updateTools()
        invalidateOptionsMenu()
    }

    private fun updateTools() {
        if (!isTablet) return
        val v = current?.view
        val n = v?.pageCount ?: 0
        updatingTools = true
        layoutGroup.check(
            when (v?.layoutMode) {
                PdfPageView.Layout.SINGLE -> R.id.layout_single
                PdfPageView.Layout.TWO_PAGES -> R.id.layout_two
                else -> R.id.layout_continuous
            },
        )
        updatingTools = false
        zoomLevel.text = getString(R.string.zoom_percent, v?.zoomPercent ?: 100)
        val page = v?.currentPage ?: 0
        pageLabel.text = if (n > 0) getString(R.string.page_of, page + 1, n) else ""
        pagePrev.isEnabled = n > 0 && page > 0
        pageNext.isEnabled = n > 0 && page < n - 1
    }

    private fun showZoomMenu(anchor: View) {
        val v = current?.view ?: return
        PopupMenu(this, anchor).apply {
            menu.add(Menu.NONE, ZOOM_FIT_WIDTH, 0, R.string.fit_width)
            menu.add(Menu.NONE, ZOOM_FIT_PAGE, 1, R.string.fit_page)
            for ((i, p) in intArrayOf(50, 75, 100, 125, 150, 200, 300, 400).withIndex()) {
                menu.add(Menu.NONE, ZOOM_PERCENT + p, 10 + i, getString(R.string.zoom_percent, p))
            }
            setOnMenuItemClickListener {
                when (it.itemId) {
                    ZOOM_FIT_WIDTH -> v.setFitMode(PdfPageView.Fit.WIDTH)
                    ZOOM_FIT_PAGE -> v.setFitMode(PdfPageView.Fit.PAGE)
                    else -> v.setZoomPercent(it.itemId - ZOOM_PERCENT)
                }
                true
            }
            show()
        }
    }

    /** Phones: the page number, for a moment while scrolling. */
    private fun showPageChip() {
        if (isTablet) return
        val v = current?.view ?: return
        if (v.pageCount < 2) return
        pageChip.text = getString(R.string.page_of, v.currentPage + 1, v.pageCount)
        pageChip.visibility = View.VISIBLE
        main.removeCallbacks(hidePageChip)
        main.postDelayed(hidePageChip, 1500)
    }

    private fun askPage() {
        val v = current?.view ?: return
        val n = v.pageCount
        if (n < 1) return
        val input = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_NUMBER
            hint = getString(R.string.go_to_page_hint, n)
            setText((v.currentPage + 1).toString())
            selectAll()
        }
        val pad = (20 * resources.displayMetrics.density).toInt()
        val box = FrameLayout(this).apply {
            setPadding(pad, 0, pad, 0)
            addView(input)
        }
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.go_to_page)
            .setView(box)
            .setPositiveButton(R.string.ok) { _, _ -> input.text.toString().toIntOrNull()?.let { v.goToPage(it - 1) } }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    /** Desktop shortcuts with a hardware keyboard. */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        // Enter in the search field: the next result (Shift+Enter: the
        // previous one). Handled here so the field never moves the focus.
        val enter = event.keyCode == KeyEvent.KEYCODE_ENTER || event.keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER
        if (enter && searchInput.hasFocus()) {
            if (event.action == KeyEvent.ACTION_DOWN) searchEnter(back = event.isShiftPressed)
            return true
        }
        if (event.action == KeyEvent.ACTION_DOWN && event.isCtrlPressed && handleShortcut(event)) return true
        return super.dispatchKeyEvent(event)
    }

    private fun handleShortcut(e: KeyEvent): Boolean {
        val v = current?.view
        val ch = e.getUnicodeChar(e.metaState and KeyEvent.META_CTRL_MASK.inv()).toChar().lowercaseChar()
        // Editing a text field: Ctrl+A/C/X/V and Ctrl+arrows are for the field.
        if (v?.fieldShortcut(e.keyCode, ch, e.isShiftPressed) == true) return true
        when {
            e.keyCode == KeyEvent.KEYCODE_TAB -> switchTab(if (e.isShiftPressed) -1 else 1)
            e.keyCode == KeyEvent.KEYCODE_PAGE_DOWN -> switchTab(1)
            e.keyCode == KeyEvent.KEYCODE_PAGE_UP -> switchTab(-1)
            e.keyCode == KeyEvent.KEYCODE_MOVE_HOME -> v?.goToPage(0)
            e.keyCode == KeyEvent.KEYCODE_MOVE_END -> v?.goToPage(Int.MAX_VALUE)
            e.keyCode == KeyEvent.KEYCODE_NUMPAD_ADD || ch == '+' || ch == '=' -> v?.zoomIn()
            e.keyCode == KeyEvent.KEYCODE_NUMPAD_SUBTRACT || ch == '-' -> v?.zoomOut()
            e.keyCode == KeyEvent.KEYCODE_NUMPAD_0 || ch == '0' -> v?.setFitMode(PdfPageView.Fit.WIDTH)
            ch == 's' -> if (e.isShiftPressed) saveAs() else save()
            ch == 'o' -> openDocument.launch(arrayOf("application/pdf"))
            ch == 'w' -> current?.let { closeTab(it) }
            ch == 'g' -> askPage()
            ch == 'f' -> openSearch()
            ch == 'c' -> return v?.copySelection() == true
            else -> return false
        }
        return true
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    private val runSearch = Runnable { search(searchInput.text.toString()) }
    private var searchToken = 0
    private var searchedQuery = ""
    private var hitCount = 0
    private var hitIndex = 0

    private fun setupSearch() {
        // Searches while typing, after a short pause.
        searchInput.doAfterTextChanged {
            main.removeCallbacks(runSearch)
            main.postDelayed(runSearch, 350)
        }
        // The search key of the on-screen keyboard (a hardware Enter is
        // handled in dispatchKeyEvent).
        searchInput.setOnEditorActionListener { _, actionId, _ ->
            if (actionId == EditorInfo.IME_ACTION_SEARCH) searchEnter(back = false)
            actionId == EditorInfo.IME_ACTION_SEARCH
        }
        searchInput.setOnKeyListener { _, keyCode, event ->
            if (keyCode == KeyEvent.KEYCODE_ESCAPE && event.action == KeyEvent.ACTION_DOWN) {
                closeSearch()
                true
            } else {
                false
            }
        }
        findViewById<View>(R.id.search_prev).setOnClickListener { stepHit(-1) }
        findViewById<View>(R.id.search_next).setOnClickListener { stepHit(1) }
        findViewById<View>(R.id.search_close).setOnClickListener { closeSearch() }
    }

    /** Enter in the search field: search, or go to the next (previous) result. */
    private fun searchEnter(back: Boolean) {
        main.removeCallbacks(runSearch)
        val q = searchInput.text.toString()
        if (q == searchedQuery) stepHit(if (back) -1 else 1) else search(q)
    }

    private fun openSearch() {
        if (current?.doc == null) return
        searchBar.visibility = View.VISIBLE
        searchInput.requestFocus()
        searchInput.selectAll()
        val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.showSoftInput(searchInput, InputMethodManager.SHOW_IMPLICIT)
    }

    private fun closeSearch() {
        main.removeCallbacks(runSearch)
        searchToken++
        searchedQuery = ""
        hitCount = 0
        searchCount.text = ""
        searchBar.visibility = View.GONE
        current?.view?.clearSearch()
        val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.hideSoftInputFromWindow(searchInput.windowToken, 0)
        current?.view?.requestFocus()
    }

    private fun search(query: String) {
        val t = current ?: return
        val d = t.doc ?: return
        searchedQuery = query
        val token = ++searchToken
        if (query.isBlank()) {
            hitCount = 0
            searchCount.text = ""
            t.view.clearSearch()
            return
        }
        PdfEngine.run({ d.search(query) }) { r ->
            if (token != searchToken || t !== current) return@run
            val hits = r.getOrDefault(emptyList())
            t.view.setSearchHits(hits)
            hitCount = hits.size
            if (hits.isEmpty()) {
                searchCount.setText(R.string.search_none)
                if (t.kind == DocKind.XFA_DYNAMIC) Toast.makeText(this, R.string.search_none_xfa, Toast.LENGTH_LONG).show()
            } else {
                showHit(t.view.firstHitFromCurrentPage())
            }
        }
    }

    private fun showHit(i: Int) {
        hitIndex = i
        current?.view?.showHit(i)
        searchCount.text = getString(R.string.search_count, i + 1, hitCount)
    }

    private fun stepHit(delta: Int) {
        if (hitCount == 0) return
        showHit((hitIndex + delta).mod(hitCount))
    }

    // -----------------------------------------------------------------------
    // Form events
    // -----------------------------------------------------------------------

    /** A change that is not an input on the form (a signature placed). */
    private fun markModified(t: DocTab) {
        t.edits++
        if (!t.modified) {
            t.modified = true
            updateTabLabel(t)
            if (t === current) updateTitle()
        }
    }

    /** Printing: the system's print screen, with the document as it is shown. */
    private fun print() {
        val t = current ?: return
        val d = t.doc ?: return
        t.view.commitField {
            // Printing writes the placed signatures into the document.
            Printing.print(this, d, t.item.name) { t.view.refreshSignatures() }
        }
    }

    /** The digital signatures the document has, and whether they still match. */
    private fun showDigitalSignatures() {
        val d = current?.doc ?: return
        PdfEngine.run({ Pair(d.digitalSignatures(), d.hasUsageRights()) }) { r ->
            val (list, rights) = r.getOrNull() ?: return@run
            Signatures.showDigital(this, list, rights)
        }
    }

    /** Signing: the saved signatures, or create one; then a tap places it. */
    private fun sign() {
        val t = current ?: return
        val d = t.doc ?: return
        PdfEngine.run({ d.canSign() }) { r ->
            val handwritten = r.getOrNull() == true
            Signatures.showMenu(this, handwritten, onPlace = { id ->
                t.view.startPlacingSignature(Signatures.library(this), id)
                Toast.makeText(this, R.string.sign_place_hint, Toast.LENGTH_LONG).show()
            }, onDigital = { signDigital(t, dynamicXfa = !handwritten) })
        }
    }

    // -----------------------------------------------------------------------
    // Digital signature with a certificate
    // -----------------------------------------------------------------------

    /** Who signs: the certificate chain (signer first) and the signing operation. */
    private class CertSigner(val chain: List<ByteArray>, val sign: (ByteArray) -> ByteArray)

    /** Options, then the certificate, the place on the page, where to save, and signing. */
    private fun signDigital(t: DocTab, dynamicXfa: Boolean) {
        val d = t.doc ?: return
        val lib = Signatures.library(this)
        t.view.commitField {
            PdfEngine.run({ Pair(d.signatureFields(), lib.list()) }) { r ->
                val (fields, saved) = r.getOrElse { e -> return@run showError(e.message ?: "") }
                DigitalSign.showOptions(this, fields, saved, dynamicXfa) { o ->
                    chooseCertificate(o.source) { signer -> placeDigital(t, o, signer) }
                }
            }
        }
    }

    private fun chooseCertificate(source: DigitalSign.Source, then: (CertSigner) -> Unit) {
        when (source) {
            DigitalSign.Source.DEVICE -> KeyChain.choosePrivateKeyAlias(this, { alias ->
                if (alias == null) return@choosePrivateKeyAlias
                Io.run({
                    val chain = KeyChain.getCertificateChain(this, alias)?.map { it.encoded } ?: error(getString(R.string.dsig_no_key))
                    val key = KeyChain.getPrivateKey(this, alias) ?: error(getString(R.string.dsig_no_key))
                    val alg = if (certificateIsRsa(chain[0])) "SHA256withRSA" else "SHA256withECDSA"
                    CertSigner(chain) { data ->
                        java.security.Signature.getInstance(alg).run {
                            initSign(key)
                            update(data)
                            sign()
                        }
                    }
                }) { s -> s.onSuccess(then).onFailure { e -> showError(getString(R.string.dsig_failed, e.message ?: "")) } }
            }, arrayOf("RSA", "EC"), null, null, null)
            DigitalSign.Source.FILE -> {
                certificateCallback = { uri ->
                    if (uri != null) {
                        val name = displayName(uri)
                        Io.run({ contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: error(name) }) { r ->
                            r.onSuccess { data -> openCertificateFile(name, data, wrong = false, then) }
                                .onFailure { e -> showError(getString(R.string.dsig_failed, e.message ?: "")) }
                        }
                    }
                }
                pickCertificate.launch(arrayOf("application/x-pkcs12", "application/pkcs12", "application/octet-stream", "*/*"))
            }
        }
    }

    private fun openCertificateFile(name: String, data: ByteArray, wrong: Boolean, then: (CertSigner) -> Unit) {
        DigitalSign.askPassword(this, name, wrong) { password ->
            Io.run({ P12Certificate.open(data, password) }) { r ->
                r.onSuccess { p12 -> then(CertSigner(p12.chain()) { p12.sign(it) }) }.onFailure { e ->
                    if (e is PdfException.WrongPassword) {
                        openCertificateFile(name, data, wrong = true, then)
                    } else {
                        showError(getString(R.string.dsig_failed, e.message ?: ""))
                    }
                }
            }
        }
    }

    /** Where the signature goes: a tap on the page if it is a new visible one. */
    private fun placeDigital(t: DocTab, o: DigitalSign.Options, signer: CertSigner) {
        when {
            !o.visible -> saveSigned(t, o, DigitalPlacement.Invisible, signer)
            o.field != null -> saveSigned(t, o, DigitalPlacement.Field(o.field), signer)
            else -> {
                Toast.makeText(this, R.string.dsig_place_hint, Toast.LENGTH_LONG).show()
                t.view.startPlacingDigital { page, x, y ->
                    val (w, h) = if (o.handwrittenId != null) Pair(240f, 66f) else Pair(200f, 54f)
                    saveSigned(t, o, DigitalPlacement.New(page.toUInt(), x, y, w, h), signer)
                }
            }
        }
    }

    /** Asks where to save the signed copy, signs and opens it in place of the document. */
    private fun saveSigned(t: DocTab, o: DigitalSign.Options, placement: DigitalPlacement, signer: CertSigner) {
        val d = t.doc ?: return
        val stem = t.item.name.substringBeforeLast('.')
        signedCallback = cb@{ uri ->
            if (uri == null || t !in docs) return@cb
            val target = itemFor(uri)
            val lib = Signatures.library(this)
            PdfEngine.run({
                d.prepareDigitalSignature(placement, o.reason, o.location, lib, o.handwrittenId, signer.chain)
            }) { r ->
                val pending = r.getOrElse { e -> return@run showError(getString(R.string.dsig_failed, e.message ?: "")) }
                t.view.refreshSignatures()
                Io.run({
                    val bytes = pending.finish(signer.sign(pending.dataToSign()))
                    writeDocument(target, bytes)
                    bytes.size.toLong()
                }) { w ->
                    w.onSuccess { size ->
                        remember(target, size)
                        // The signed copy has the changes: the tab shows it now.
                        t.modified = false
                        removeTab(t)
                        open(target)
                        Toast.makeText(this, R.string.dsig_done, Toast.LENGTH_LONG).show()
                    }.onFailure { e -> showError(getString(R.string.dsig_failed, e.message ?: "")) }
                }
            }
        }
        createSigned.launch(getString(R.string.dsig_signed_name, stem))
    }

    private fun onFormInput(t: DocTab, r: InputResult) {
        if (r.modified) t.edits++
        if (r.modified && !t.modified) {
            t.modified = true
            updateTabLabel(t)
            if (t === current) updateTitle()
        }
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
        val d = current?.doc ?: return
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

    /** PDFs open in a new tab; other files in their app. */
    private fun openAttachmentFile(file: File) {
        val ext = file.extension.lowercase()
        if (ext == "pdf") {
            open(DocItem(file.name, path = file.absolutePath))
            return
        }
        val uri = FileProvider.getUriForFile(this, "$packageName.files", file)
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
        val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        try {
            startActivity(Intent.createChooser(view, file.name))
        } catch (e: Exception) {
            Toast.makeText(this, R.string.no_app, Toast.LENGTH_SHORT).show()
        }
    }

    /**
     * Shares the document as it is shown, with the data filled in even if it
     * has not been saved: a copy in the cache, sent to the app the user picks.
     */
    private fun share() {
        val t = current ?: return
        val d = t.doc ?: return
        val failed = { e: Throwable -> showError(getString(R.string.share_error, e.message ?: e.toString())) }
        t.view.commitField {
            PdfEngine.run({ d.saveBytes() }) { r ->
                r.onSuccess { bytes ->
                    runCatching {
                        val file = Share.target(this, t.item.name)
                        file.writeBytes(bytes)
                        Share.pdf(this, file)
                    }.onFailure(failed)
                }.onFailure(failed)
            }
        }
    }

    private fun copyPageText() {
        val t = current ?: return
        val d = t.doc ?: return
        PdfEngine.run({ (0u until d.pageCount()).joinToString("\n") { d.pageText(it) } }) { r ->
            val text = r.getOrNull().orEmpty().trim()
            if (text.isEmpty()) {
                Toast.makeText(this, R.string.no_text, Toast.LENGTH_SHORT).show()
                return@run
            }
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            cm.setPrimaryClip(ClipData.newPlainText(t.item.name, text))
            Toast.makeText(this, R.string.copied, Toast.LENGTH_SHORT).show()
        }
    }

    // -----------------------------------------------------------------------

    private fun updateTitle() {
        // Tablets show the names in the tabs.
        if (isTablet) return
        val t = current
        title = when {
            t == null -> getString(R.string.app_name)
            t.loading -> getString(R.string.opening, t.item.name)
            t.modified -> "● ${t.item.name}"
            else -> t.item.name
        }
    }

    /** Explains why a document could not be saved. */
    private fun saveFailed(t: DocTab, e: Throwable) {
        val b = MaterialAlertDialogBuilder(this).setTitle(R.string.error_save_title)
        if (e is SecurityException || e is FileNotFoundException) {
            // No permission to write there (any more): somewhere else.
            b.setMessage(getString(R.string.error_save_no_access, t.item.name))
                .setPositiveButton(R.string.save_as) { _, _ -> saveAs(t) }
                .setNegativeButton(R.string.cancel, null)
        } else {
            b.setMessage(getString(R.string.save_error, e.message ?: e.toString()))
                .setPositiveButton(R.string.ok, null)
        }
        b.show()
    }

    private fun showError(message: String) {
        MaterialAlertDialogBuilder(this).setMessage(message).setPositiveButton(R.string.ok, null).show()
    }

    override fun onDestroy() {
        super.onDestroy()
        main.removeCallbacks(hidePageChip)
        main.removeCallbacks(runSearch)
        for (t in docs) {
            t.view.setDocument(null)
            release(t)
        }
        docs.clear()
    }

    companion object {
        /** Path of a file on the device to open. */
        const val EXTRA_PATH = "io.github.blzkz.formpdfreader.PATH"

        /** Opens the documents with changes kept from a previous run (see [Drafts]). */
        const val EXTRA_RECOVER = "io.github.blzkz.formpdfreader.RECOVER"

        private const val ZOOM_FIT_WIDTH = 1
        private const val ZOOM_FIT_PAGE = 2
        private const val ZOOM_PERCENT = 1000

        fun intentFor(c: Context, item: DocItem): Intent =
            if (item.path != null) {
                Intent(c, MainActivity::class.java).putExtra(EXTRA_PATH, item.path)
            } else {
                intentFor(c, requireNotNull(item.contentUri))
            }

        fun intentFor(c: Context, uri: Uri): Intent = Intent(Intent.ACTION_VIEW, uri, c, MainActivity::class.java)
    }
}
