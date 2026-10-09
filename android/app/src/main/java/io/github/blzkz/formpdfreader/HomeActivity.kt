package io.github.blzkz.formpdfreader

import android.content.Intent
import android.os.Bundle
import android.view.View
import android.widget.RadioGroup
import android.widget.TextView
import android.widget.Toast
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.view.WindowCompat
import androidx.core.view.isVisible
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.bottomnavigation.BottomNavigationView
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.materialswitch.MaterialSwitch
import com.google.android.material.navigation.NavigationBarView
import com.google.android.material.navigationrail.NavigationRailView
import java.io.File

/**
 * Start screen: recently opened documents (Home) and the settings, with a
 * bottom navigation bar (a side rail on tablets). Documents are opened with the system picker, which
 * needs no storage permission; the picker's grant is kept for the recents.
 */
class HomeActivity : AppCompatActivity() {

    private lateinit var toolbar: MaterialToolbar
    private lateinit var bottomNav: BottomNavigationView
    private lateinit var rail: NavigationRailView

    /** The navigation in use: the rail on tablets, the bottom bar on phones. */
    private lateinit var nav: NavigationBarView
    private var isTablet = false
    private lateinit var homeView: View
    private lateinit var settingsView: View
    private lateinit var fab: View
    private lateinit var recents: RecyclerView
    private lateinit var recentsEmpty: View

    private val adapter = DocAdapter(R.layout.item_doc_card, ::openItem, ::onLongPress)

    private val backToHome = object : OnBackPressedCallback(false) {
        override fun handleOnBackPressed() {
            nav.selectedItemId = R.id.nav_home
        }
    }

    private val openDocument = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) startActivity(MainActivity.intentFor(this, uri))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        setContentView(R.layout.activity_home)

        toolbar = findViewById(R.id.toolbar)
        bottomNav = findViewById(R.id.bottom_nav)
        rail = findViewById(R.id.nav_rail)
        homeView = findViewById(R.id.home_view)
        settingsView = findViewById(R.id.settings_view)
        fab = findViewById(R.id.fab_open)
        recents = findViewById(R.id.recents)
        recentsEmpty = findViewById(R.id.recents_empty)

        isTablet = resources.configuration.smallestScreenWidthDp >= 600
        if (isTablet) {
            rail.visibility = View.VISIBLE
            bottomNav.visibility = View.GONE
        }
        nav = if (isTablet) rail else bottomNav
        applySystemInsets(findViewById(R.id.root), if (isTablet) null else bottomNav)

        // As many columns as fit.
        val columnDp = resources.getDimension(R.dimen.card_column_width) / resources.displayMetrics.density
        val widthDp = resources.configuration.screenWidthDp - if (isTablet) 80 else 0
        recents.layoutManager = GridLayoutManager(this, maxOf(2, (widthDp / columnDp).toInt()))
        recents.adapter = adapter

        val open = View.OnClickListener { openDocument.launch(arrayOf("application/pdf")) }
        fab.setOnClickListener(open)
        rail.headerView?.findViewById<View>(R.id.rail_fab)?.setOnClickListener(open)

        setupSettings()

        onBackPressedDispatcher.addCallback(this, backToHome)
        nav.setOnItemSelectedListener { item ->
            showTab(item.itemId)
            true
        }
        val tab = savedInstanceState?.getInt(KEY_TAB) ?: R.id.nav_home
        nav.selectedItemId = tab
        showTab(tab)
    }

    override fun onResume() {
        super.onResume()
        // Back from the viewer, which adds what it opens.
        loadRecents()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putInt(KEY_TAB, nav.selectedItemId)
    }

    private fun showTab(id: Int) {
        val home = id == R.id.nav_home
        homeView.isVisible = home
        settingsView.isVisible = !home
        // Tablets have "Open PDF" on the rail.
        fab.isVisible = home && !isTablet
        toolbar.title = getString(if (home) R.string.app_name else R.string.nav_settings)
        backToHome.isEnabled = !home
    }

    // -----------------------------------------------------------------------
    // Home
    // -----------------------------------------------------------------------

    private fun loadRecents() {
        val items = Recents.list(this).filter { it.path == null || File(it.path).exists() }
        adapter.items = items
        recents.isVisible = items.isNotEmpty()
        recentsEmpty.isVisible = items.isEmpty()
    }

    private fun onLongPress(item: DocItem) {
        MaterialAlertDialogBuilder(this)
            .setTitle(item.name)
            .setItems(arrayOf(getString(R.string.remove_from_recents))) { _, _ ->
                Recents.remove(this, item.key)
                loadRecents()
            }
            .show()
    }

    private fun openItem(item: DocItem) {
        if (item.path != null && !File(item.path).exists()) {
            Toast.makeText(this, R.string.file_missing, Toast.LENGTH_SHORT).show()
            Recents.remove(this, item.key)
            loadRecents()
            return
        }
        startActivity(MainActivity.intentFor(this, item))
    }

    // -----------------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------------

    private fun setupSettings() {
        val theme = findViewById<RadioGroup>(R.id.theme_group)
        theme.check(
            when (Prefs.themeMode(this)) {
                AppCompatDelegate.MODE_NIGHT_NO -> R.id.theme_light
                AppCompatDelegate.MODE_NIGHT_YES -> R.id.theme_dark
                else -> R.id.theme_system
            },
        )
        theme.setOnCheckedChangeListener { _, id ->
            val mode = when (id) {
                R.id.theme_light -> AppCompatDelegate.MODE_NIGHT_NO
                R.id.theme_dark -> AppCompatDelegate.MODE_NIGHT_YES
                else -> AppCompatDelegate.MODE_NIGHT_FOLLOW_SYSTEM
            }
            // Recreates the activities with the new theme.
            if (mode != Prefs.themeMode(this)) Prefs.setThemeMode(this, mode)
        }

        val continuous = findViewById<MaterialSwitch>(R.id.continuous_switch)
        continuous.isChecked = Prefs.continuousXfa(this)
        continuous.setOnCheckedChangeListener { _, checked -> Prefs.setContinuousXfa(this, checked) }

        findViewById<TextView>(R.id.about_text).text = getString(R.string.about_text, appVersion())
        findViewById<View>(R.id.open_licenses).setOnClickListener {
            startActivity(Intent(this, LicensesActivity::class.java))
        }
    }

    private fun appVersion(): String =
        runCatching { packageManager.getPackageInfo(packageName, 0).versionName }.getOrNull() ?: ""

    companion object {
        private const val KEY_TAB = "tab"
    }
}
