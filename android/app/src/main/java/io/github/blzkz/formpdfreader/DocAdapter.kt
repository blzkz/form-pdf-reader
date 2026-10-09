package io.github.blzkz.formpdfreader

import android.annotation.SuppressLint
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.ImageView
import android.widget.TextView
import androidx.recyclerview.widget.RecyclerView

/** Documents as cards (carousel) or rows (lists), with their thumbnail. */
class DocAdapter(
    private val layout: Int,
    private val onClick: (DocItem) -> Unit,
    private val onLongClick: ((DocItem) -> Unit)? = null,
) : RecyclerView.Adapter<DocAdapter.Holder>() {

    var items: List<DocItem> = emptyList()
        @SuppressLint("NotifyDataSetChanged")
        set(value) {
            field = value
            notifyDataSetChanged()
        }

    class Holder(v: View) : RecyclerView.ViewHolder(v) {
        val thumb: ImageView = v.findViewById(R.id.thumb)
        val name: TextView = v.findViewById(R.id.name)
        val details: TextView? = v.findViewById(R.id.details)
    }

    override fun getItemCount() = items.size

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int) =
        Holder(LayoutInflater.from(parent.context).inflate(layout, parent, false))

    override fun onBindViewHolder(h: Holder, position: Int) {
        val item = items[position]
        h.name.text = item.name
        h.details?.text = item.details(h.itemView.context)
        Thumbnails.load(h.thumb, item)
        h.itemView.setOnClickListener { onClick(item) }
        val long = onLongClick
        if (long == null) {
            h.itemView.setOnLongClickListener(null)
        } else {
            h.itemView.setOnLongClickListener {
                long(item)
                true
            }
        }
    }
}
