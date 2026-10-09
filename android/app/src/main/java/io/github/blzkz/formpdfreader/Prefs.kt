package io.github.blzkz.formpdfreader

import android.content.Context
import androidx.appcompat.app.AppCompatDelegate

/** App settings (the Settings tab). */
object Prefs {
    private const val THEME = "theme"
    private const val CONTINUOUS_XFA = "continuous_xfa"
    private const val VIEW_LAYOUT = "view_layout"

    private fun prefs(c: Context) = c.getSharedPreferences("settings", Context.MODE_PRIVATE)

    /** One of the AppCompatDelegate.MODE_NIGHT_* values. */
    fun themeMode(c: Context): Int =
        prefs(c).getInt(THEME, AppCompatDelegate.MODE_NIGHT_FOLLOW_SYSTEM)

    fun setThemeMode(c: Context, mode: Int) {
        prefs(c).edit().putInt(THEME, mode).apply()
        AppCompatDelegate.setDefaultNightMode(mode)
    }

    /** Shows dynamic XFA forms as one continuous page (as on the desktop). */
    fun continuousXfa(c: Context): Boolean = prefs(c).getBoolean(CONTINUOUS_XFA, true)

    fun setContinuousXfa(c: Context, value: Boolean) {
        prefs(c).edit().putBoolean(CONTINUOUS_XFA, value).apply()
    }

    /** Page layout of the viewer (the last one chosen). */
    fun viewLayout(c: Context): PdfPageView.Layout =
        runCatching { PdfPageView.Layout.valueOf(prefs(c).getString(VIEW_LAYOUT, null)!!) }.getOrDefault(PdfPageView.Layout.CONTINUOUS)

    fun setViewLayout(c: Context, layout: PdfPageView.Layout) {
        prefs(c).edit().putString(VIEW_LAYOUT, layout.name).apply()
    }
}
