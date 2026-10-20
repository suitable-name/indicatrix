package com.indicatrixsuite.mobile

import android.os.Build
import android.view.WindowInsets
import android.view.inputmethod.InputMethodManager
import android.window.BackEvent
import android.window.OnBackAnimationCallback
import android.window.OnBackInvokedCallback
import android.window.OnBackInvokedDispatcher
import androidx.annotation.RequiresApi

/** One API 33+ app owner. Never registered at the app root or over an open IME. */
@RequiresApi(33)
internal class BackNavigationController(
    private val activity: MainActivity,
    private val commit: (Int, Int) -> Unit,
    private val progress: (Int, Int, Float) -> Unit,
) {
    private var target = 0
    private var version = 0
    private var registered = false
    private var gesture: Pair<Int, Int>? = null
    private val callback: OnBackInvokedCallback = if (Build.VERSION.SDK_INT >= 34) {
        AnimationCallback()
    } else {
        OnBackInvokedCallback { invoke() }
    }
    private val layoutListener = android.view.ViewTreeObserver.OnGlobalLayoutListener { updateRegistration() }

    init { activity.window.decorView.viewTreeObserver.addOnGlobalLayoutListener(layoutListener) }

    /** Mirrors Rust's Back state (`MainActivity.setBackTarget`); UI thread only. */
    fun setTarget(newTarget: Int, newVersion: Int) {
        target = newTarget
        version = newVersion
        updateRegistration()
    }

    private fun imeVisible(): Boolean = activity.window.decorView.rootWindowInsets
        ?.isVisible(WindowInsets.Type.ime()) == true

    private fun updateRegistration() {
        val needed = target != 0 && !imeVisible()
        if (needed == registered) return
        if (needed) {
            activity.onBackInvokedDispatcher.registerOnBackInvokedCallback(
                OnBackInvokedDispatcher.PRIORITY_DEFAULT, callback,
            )
        } else {
            activity.onBackInvokedDispatcher.unregisterOnBackInvokedCallback(callback)
        }
        registered = needed
    }

    private fun invoke() {
        // An IME appearing between registration and commit consumes this action alone.
        if (imeVisible()) {
            activity.getSystemService(InputMethodManager::class.java)
                ?.hideSoftInputFromWindow(activity.window.decorView.windowToken, 0)
            gesture = null
            return
        }
        val expected = gesture ?: Pair(target, version)
        gesture = null
        progress(expected.first, expected.second, 0f)
        if (expected.first != 0) commit(expected.first, expected.second)
    }

    @RequiresApi(34)
    private inner class AnimationCallback : OnBackAnimationCallback {
        override fun onBackStarted(event: BackEvent) {
            gesture = Pair(target, version)
            progress(target, version, 0f)
        }
        override fun onBackProgressed(event: BackEvent) {
            gesture?.let { progress(it.first, it.second, event.progress) }
        }
        override fun onBackCancelled() {
            gesture?.let { progress(it.first, it.second, 0f) }
            gesture = null
        }
        override fun onBackInvoked() { invoke() }
    }

    fun close() {
        gesture?.let { progress(it.first, it.second, 0f) }
        gesture = null
        if (registered) activity.onBackInvokedDispatcher.unregisterOnBackInvokedCallback(callback)
        registered = false
        activity.window.decorView.viewTreeObserver.removeOnGlobalLayoutListener(layoutListener)
    }
}
