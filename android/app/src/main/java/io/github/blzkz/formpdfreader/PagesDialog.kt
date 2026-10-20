package io.github.blzkz.formpdfreader

import android.app.Dialog
import android.graphics.Bitmap
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.ItemTouchHelper
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.card.MaterialCardView
import java.nio.ByteBuffer
import kotlin.math.max
import uniffi.form_pdf_reader_ffi.PageEditOp
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * Organise pages: the thumbnails of every page. A tap selects pages; a long
 * press and drag moves one (or the selected ones); the toolbar rotates,
 * deletes or extracts the selected pages, and inserts the pages of another
 * PDF or of a scan after them.
 *
 * Every change is applied at once by [onEdit], which reopens the document
 * and gives it back with [setDocument].
 */
class PagesDialog(
    private val activity: AppCompatActivity,
    private var doc: PdfDocument,
    signed: Boolean,
    private val onEdit: (PageEditOp, List<Int>) -> Unit,
    private val onExtract: (List<Int>) -> Unit,
    private val onInsert: (at: Int) -> Unit,
    private val onScan: (at: Int) -> Unit,
) {
    private val dialog = Dialog(activity, R.style.Theme_FormPdfReader)
    private val toolbar: MaterialToolbar
    private val grid: RecyclerView
    private val info: TextView
    private val density = activity.resources.displayMetrics.density

    /** The page each position shows (it changes while one is dragged). */
    private var order = mutableListOf<Int>()
    private val selected = sortedSetOf<Int>()
    private val thumbs = HashMap<Int, Bitmap>()
    private var generation = 0
    private val adapter = Adapter()

    init {
        dialog.setContentView(R.layout.dialog_pages)
        toolbar = dialog.findViewById(R.id.pages_toolbar)
        grid = dialog.findViewById(R.id.pages_grid)
        info = dialog.findViewById(R.id.pages_info)
        info.text = if (signed) {
            activity.getString(R.string.pages_signed_warning) + "\n" + activity.getString(R.string.pages_hint)
        } else {
            activity.getString(R.string.pages_hint)
        }
        toolbar.setNavigationOnClickListener { dialog.dismiss() }
        toolbar.inflateMenu(R.menu.pages)
        toolbar.setOnMenuItemClickListener { item ->
            val sel = selected.toList()
            val at = (sel.maxOrNull() ?: (order.size - 1)) + 1
            when (item.itemId) {
                R.id.pages_rotate_left -> if (sel.isNotEmpty()) onEdit(PageEditOp.Rotate(sel.map { it.toUInt() }, -1), sel)
                R.id.pages_rotate_right -> if (sel.isNotEmpty()) onEdit(PageEditOp.Rotate(sel.map { it.toUInt() }, 1), sel)
                R.id.pages_delete -> when {
                    sel.isEmpty() -> Unit
                    sel.size >= order.size -> android.widget.Toast.makeText(activity, R.string.pages_delete_all, android.widget.Toast.LENGTH_SHORT).show()
                    else -> onEdit(PageEditOp.Delete(sel.map { it.toUInt() }), listOf(sel.min().coerceAtMost(order.size - 1 - sel.size)))
                }
                R.id.pages_extract -> if (sel.isNotEmpty()) onExtract(sel)
                R.id.pages_insert -> onInsert(at)
                R.id.pages_scan -> onScan(at)
                R.id.pages_select_all -> {
                    selected.clear()
                    selected.addAll(order.indices)
                    adapter.notifyDataSetChanged()
                    updateMenu()
                }
            }
            true
        }
        val widthDp = activity.resources.configuration.screenWidthDp
        grid.layoutManager = GridLayoutManager(activity, max(2, widthDp / 120))
        grid.adapter = adapter
        ItemTouchHelper(object : ItemTouchHelper.SimpleCallback(ItemTouchHelper.UP or ItemTouchHelper.DOWN or ItemTouchHelper.LEFT or ItemTouchHelper.RIGHT, 0) {
            private var from = -1
            private var to = -1

            override fun onMove(rv: RecyclerView, vh: RecyclerView.ViewHolder, target: RecyclerView.ViewHolder): Boolean {
                val a = vh.bindingAdapterPosition
                val b = target.bindingAdapterPosition
                if (from < 0) from = a
                to = b
                order.add(b, order.removeAt(a))
                adapter.notifyItemMoved(a, b)
                return true
            }

            override fun onSwiped(vh: RecyclerView.ViewHolder, direction: Int) = Unit

            override fun clearView(rv: RecyclerView, vh: RecyclerView.ViewHolder) {
                super.clearView(rv, vh)
                val (a, b) = from to to
                from = -1
                to = -1
                if (a < 0 || b < 0 || a == b) return
                // The dragged page, or the selected ones if it was one of them.
                val pages = if (a in selected) selected.toList() else listOf(a)
                val dest = if (pages.size == 1) b else (b - pages.count { it < b && it != a }).coerceAtLeast(0)
                onEdit(PageEditOp.Move(pages.map { it.toUInt() }, dest.toUInt()), (dest until dest + pages.size).toList())
            }
        }).attachToRecyclerView(grid)
        load(listOf(0))
    }

    fun show() = dialog.show()

    val isShowing: Boolean get() = dialog.isShowing

    /** The document changed: show its pages, with these selected. */
    fun setDocument(d: PdfDocument, select: List<Int>) {
        doc = d
        load(select)
    }

    private fun load(select: List<Int>) {
        val d = doc
        val gen = ++generation
        thumbs.clear()
        PdfEngine.run({ d.pageCount().toInt() }) { r ->
            if (gen != generation) return@run
            val n = r.getOrDefault(0)
            order = (0 until n).toMutableList()
            selected.clear()
            selected.addAll(select.filter { it in 0 until n })
            adapter.notifyDataSetChanged()
            updateMenu()
        }
    }

    private fun updateMenu() {
        val any = selected.isNotEmpty()
        for (id in intArrayOf(R.id.pages_rotate_left, R.id.pages_rotate_right, R.id.pages_delete, R.id.pages_extract)) {
            toolbar.menu.findItem(id)?.isEnabled = any
        }
        toolbar.subtitle = if (any) activity.getString(R.string.pages_selected, selected.size) else null
    }

    private fun requestThumb(page: Int) {
        val d = doc
        val gen = generation
        val w = (110 * density).toInt()
        PdfEngine.run({
            val s = d.pageSize(page.toUInt())
            val h = (w * s.height / s.width).toInt().coerceIn(1, w * 4)
            Triple(w, h, d.render(page.toUInt(), w, h, 0, 0, w, h))
        }) { r ->
            if (gen != generation) return@run
            val (bw, bh, px) = r.getOrNull() ?: return@run
            if (px.size != bw * bh * 4) return@run
            val bmp = Bitmap.createBitmap(bw, bh, Bitmap.Config.ARGB_8888)
            bmp.copyPixelsFromBuffer(ByteBuffer.wrap(px))
            thumbs[page] = bmp
            val pos = order.indexOf(page)
            if (pos >= 0) adapter.notifyItemChanged(pos)
        }
    }

    private class Holder(v: View) : RecyclerView.ViewHolder(v) {
        val card: MaterialCardView = v.findViewById(R.id.thumb_card)
        val image: ImageView = v.findViewById(R.id.thumb_image)
        val number: TextView = v.findViewById(R.id.thumb_number)
    }

    private inner class Adapter : RecyclerView.Adapter<Holder>() {
        override fun getItemCount() = order.size

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int) =
            Holder(LayoutInflater.from(parent.context).inflate(R.layout.item_page_thumb, parent, false))

        override fun onBindViewHolder(h: Holder, position: Int) {
            val page = order[position]
            h.number.text = (position + 1).toString()
            val bmp = thumbs[page]
            h.image.setImageBitmap(bmp)
            if (bmp == null) requestThumb(page)
            h.card.isChecked = page in selected
            h.card.contentDescription = activity.getString(R.string.page_of, position + 1, order.size)
            h.card.setOnClickListener {
                val p = order[h.bindingAdapterPosition]
                if (!selected.remove(p)) selected.add(p)
                h.card.isChecked = p in selected
                updateMenu()
            }
        }
    }
}
