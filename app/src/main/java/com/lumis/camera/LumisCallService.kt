package com.lumis.camera

import android.content.ComponentName
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.ConnectionService
import android.telecom.DisconnectCause
import android.telecom.PhoneAccount
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Log

// "I'm on a call, don't bother me." While Lumis is focused we register a SELF-MANAGED Telecom call (the same
// mechanism VoIP apps like Messenger use). The system then treats incoming notifications as during-a-call -
// no sound, no vibration - while media volume is untouched. No Do-Not-Disturb special access is needed
// (only MANAGE_OWN_CALLS, a normal install-time permission). Crucially the call session is owned by THIS
// process, so if the app dies (auto-nuke, crash, OS kill) Telecom auto-releases it and the phone's normal
// alerting restores immediately - the "volumes snap back the instant I nuke it" behaviour, for free.

/// The self-managed connection. It carries no real audio; it exists only so the system sees an active call.
class SilentConnection : Connection() {
    init {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            connectionProperties = PROPERTY_SELF_MANAGED
            audioModeIsVoip = true
            // We never actually emit/receive audio; keep capabilities minimal.
            connectionCapabilities = 0
        }
    }

    override fun onShowIncomingCallUi() {}
    override fun onStateChanged(state: Int) {}

    override fun onDisconnect() {
        setDisconnected(DisconnectCause(DisconnectCause.LOCAL))
        destroy()
        if (CallSilencer.current === this) {
            CallSilencer.current = null
        }
    }

    override fun onAbort() {
        onDisconnect()
    }
}

/// Telecom binds to this when we place our self-managed call; it hands back a SilentConnection. Runs in the
/// main (UI) process - no foreground service needed; the active call session does the silencing.
class LumisConnectionService : ConnectionService() {
    override fun onCreateOutgoingConnection(
        connectionManagerPhoneAccount: PhoneAccountHandle?,
        request: ConnectionRequest?
    ): Connection {
        val conn = SilentConnection()
        CallSilencer.current = conn
        conn.setActive() // mark the call live so notifications are suppressed
        Log.i("LumisCall", "Self-managed silence call active")
        return conn
    }

    override fun onCreateOutgoingConnectionFailed(
        connectionManagerPhoneAccount: PhoneAccountHandle?,
        request: ConnectionRequest?
    ) {
        Log.w("LumisCall", "Outgoing self-managed connection failed")
        CallSilencer.current = null
    }
}

/// Focus-driven control: register the PhoneAccount once, place the silent call when Lumis gains focus, end it
/// when it backgrounds. All best-effort and exception-guarded - silencing must never crash the camera.
object CallSilencer {
    @Volatile
    var current: SilentConnection? = null

    private const val ACCOUNT_ID = "lumis_silence"

    private fun handle(context: Context): PhoneAccountHandle =
        PhoneAccountHandle(ComponentName(context, LumisConnectionService::class.java), ACCOUNT_ID)

    /// Register the self-managed PhoneAccount (idempotent - safe to call every launch).
    fun register(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        try {
            val tm = context.getSystemService(Context.TELECOM_SERVICE) as TelecomManager
            val account = PhoneAccount.builder(handle(context), "Lumis")
                .setCapabilities(PhoneAccount.CAPABILITY_SELF_MANAGED)
                .build()
            tm.registerPhoneAccount(account)
        } catch (e: Exception) {
            Log.w("LumisCall", "registerPhoneAccount failed: ${e.javaClass.simpleName}: ${e.message}")
        }
    }

    /// Place the silent self-managed call so the system suppresses notification sound + vibration. No-op if
    /// one is already active or the platform is too old.
    fun start(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        if (current != null) return
        try {
            val tm = context.getSystemService(Context.TELECOM_SERVICE) as TelecomManager
            val extras = Bundle().apply {
                putParcelable(TelecomManager.EXTRA_PHONE_ACCOUNT_HANDLE, handle(context))
                putBoolean(TelecomManager.EXTRA_START_CALL_WITH_SPEAKERPHONE, false)
            }
            // Self-addressed URI; the call never connects anywhere, it just exists.
            val uri = Uri.fromParts("tel", "lumis", null)
            tm.placeCall(uri, extras)
        } catch (e: SecurityException) {
            Log.w("LumisCall", "placeCall denied (MANAGE_OWN_CALLS?): ${e.message}")
        } catch (e: Exception) {
            Log.w("LumisCall", "placeCall failed: ${e.javaClass.simpleName}: ${e.message}")
        }
    }

    /// End the silent call (called on background) so normal alerting resumes immediately. If the process dies
    /// before this runs, Telecom auto-releases the call anyway.
    fun stop() {
        val c = current ?: return
        try {
            c.setDisconnected(DisconnectCause(DisconnectCause.LOCAL))
            c.destroy()
        } catch (e: Exception) {
            Log.w("LumisCall", "stop failed: ${e.javaClass.simpleName}: ${e.message}")
        }
        current = null
    }
}
