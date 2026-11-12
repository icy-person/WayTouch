package com.waytouch

import android.annotation.TargetApi
import android.app.Application
import android.app.Instrumentation
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import android.os.Looper
import android.os.Process

private open class ShellContextWrapper(base: Context) : ContextWrapper(base) {
    override fun getPackageName(): String = "com.android.shell"
    override fun getOpPackageName(): String = "com.android.shell"
    override fun getApplicationContext(): Context = this
    override fun getSystemService(name: String): Any? {
        val service = super.getSystemService(name) ?: return null
        if (name == Context.INPUT_SERVICE) {
            try {
                service.javaClass.getDeclaredField("mContext").apply {
                    isAccessible = true
                    set(service, this@ShellContextWrapper)
                }
            } catch (t: Throwable) {
                System.err.println("WTCH input context binding warning: $t")
            }
        }
        return service
    }
}

/**
 * Keep the API-31-only type reference in a separate class. On Android 9–11
 * this class is never loaded, so verifier/linker resolution cannot fail merely
 * because AttributionSource is absent from the platform.
 */
@TargetApi(31)
private class ShellContextWrapperApi31(base: Context) : ShellContextWrapper(base) {
    override fun getAttributionSource(): android.content.AttributionSource =
        android.content.AttributionSource.Builder(Process.SHELL_UID)
            .setPackageName("com.android.shell")
            .build()
}

/** Minimal shell-attributed Context bootstrap inspired by scrcpy FakeContext/Workarounds. */
object ShellContext {
    @Suppress("DEPRECATION")
    fun create(): Context {
        if (Looper.myLooper() == null) Looper.prepareMainLooper()
        val threadClass = Class.forName("android.app.ActivityThread")
        val thread = threadClass.getDeclaredConstructor().apply { isAccessible = true }.newInstance()

        threadClass.getDeclaredField("sCurrentActivityThread").apply {
            isAccessible = true
            set(null, thread)
        }
        threadClass.getDeclaredField("mSystemThread").apply {
            isAccessible = true
            setBoolean(thread, true)
        }

        if (Build.VERSION.SDK_INT >= 31) {
            try {
                val internalClass = Class.forName("android.app.ActivityThreadInternal")
                val configClass = Class.forName("android.app.ConfigurationController")
                val config = configClass.getDeclaredConstructor(internalClass).apply {
                    isAccessible = true
                }.newInstance(thread)
                threadClass.getDeclaredField("mConfigurationController").apply {
                    isAccessible = true
                    set(thread, config)
                }
            } catch (t: Throwable) {
                System.err.println("WTCH config bootstrap warning: $t")
            }
        }

        val systemContext = threadClass.getDeclaredMethod("getSystemContext").apply {
            isAccessible = true
        }.invoke(thread) as Context

        val shellContext = if (Build.VERSION.SDK_INT >= 31) {
            Class.forName("com.waytouch.ShellContextWrapperApi31")
                .getDeclaredConstructor(Context::class.java)
                .apply { isAccessible = true }
                .newInstance(systemContext) as Context
        } else {
            ShellContextWrapper(systemContext)
        }
        try {
            val app = Instrumentation.newApplication(Application::class.java, shellContext)
            threadClass.getDeclaredField("mInitialApplication").apply {
                isAccessible = true
                set(thread, app)
            }
        } catch (t: Throwable) {
            System.err.println("WTCH application bootstrap warning: $t")
        }
        return shellContext
    }
}
