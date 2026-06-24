package com.lumis.camera

import android.app.Activity
import android.os.Bundle
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.Surface
import android.view.MotionEvent
import android.view.WindowManager
import android.view.KeyEvent
import android.graphics.PixelFormat
import android.os.Build
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.os.Handler
import android.os.Looper
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.content.Context
import android.os.PowerManager
import android.os.Process
import android.content.*
import android.os.*
import android.util.Log
import android.view.Choreographer
import kotlin.math.abs
import android.content.res.Configuration
import android.os.ParcelFileDescriptor
import android.os.RemoteException

// UI States for single activity app
enum class UIState {
    MENU,           // Show camera selection menu
    CAMERA_OPENING, // Show frozen menu while camera initializes
    CAMERA_READY    // Show camera UI
}

class UserInterface : Activity(), SurfaceHolder.Callback {
   private lateinit var surfaceView: SurfaceView
   
   // UI State Management
   private var currentState = UIState.MENU
   
   // Service binding for CameraInterface
   private var cameraService: Messenger? = null
   private var serviceBound = false
   
   // Native context pointers
   private var uiContextPtr: Long = 0L
   private var menuContextPtr: Long = 0L
   
   // Menu state
   private var cameraArray: FloatArray? = null
   private var selectedCameraIndex: Int = -1
   
   // Surface properties
   private var surfaceWidth: Int = 0
   private var surfaceHeight: Int = 0
   private var surfaceDensity: Int = 0
   
   // Camera data (when opened)
   private var sharedMemoryPtr: Long = 0L
   private var sharedMemoryFd: Int = -1
   private var cameraWidth: Int = 0
   private var cameraHeight: Int = 0
   
   // Input state - sent to Rust every frame.
   // currentTouchX/Y hold the latest touch position (NaN = no touch). onTouchEvent runs on the UI thread and writes these; the render loop reads them once per frame. At high resolution a frame's draw can take longer than a quick tap's DOWN..UP, so the UP would overwrite the DOWN with NaN before any frame sampled it - the tap is lost. To prevent that we latch: a DOWN that has not yet been read by a draw must survive at least one frame as a valid touch before UP clears it. touchDownSampled tracks whether the current down has been delivered to a draw; pendingTouchUp defers the NaN clear by one frame when it hasn't.
   private var currentTouchX: Float = Float.NaN
   private var currentTouchY: Float = Float.NaN
   @Volatile private var touchDownSampled: Boolean = false
   @Volatile private var pendingTouchUp: Boolean = false
   private var gravityX: Float = 0.0f
   private var gravityY: Float = 0.0f  
   private var gravityZ: Float = 0.0f
   private var bluetoothShutterPressed: Boolean = false
   private var savePressed: Boolean = false
   private var continuousSavePressed: Boolean = false
   
   // Sensors for gravity (raw values sent to Rust)
   private lateinit var sensorManager: SensorManager
   private var gravitySensor: Sensor? = null
   
   // Power management
   private lateinit var powerManager: PowerManager
   private var wakeLock: PowerManager.WakeLock? = null
   
   // Continuous rendering for camera UI
   private var renderLoopActive = false
   
   private val frameCallback = object : Choreographer.FrameCallback {
       override fun doFrame(frameTimeNanos: Long) {
           if (renderLoopActive && uiContextPtr != 0L) {
               drawUIFrame()
               Choreographer.getInstance().postFrameCallback(this)
           }
       }
   }
   
   // Auto-nuke timer for focus loss
   private var autoNukeHandler: Handler? = null
   private val autoNukeRunnable = Runnable {
       Log.i("UserInterface", "Auto-nuke timer expired - killing UI")
       moveTaskToBack(true)
       Process.killProcess(Process.myPid())
   }
   
   // Messenger for receiving messages from CameraInterface
   private val clientMessenger = Messenger(object : Handler(Looper.getMainLooper()) {
       override fun handleMessage(msg: Message) {
           when (msg.what) {
               CameraInterface.MSG_CAMERAS_ENUMERATED -> {
                   val receivedCameraArray = msg.data.getFloatArray("cameras")
                   if (receivedCameraArray != null) {
                       onCamerasEnumerated(receivedCameraArray)
                   }
               }
               CameraInterface.MSG_ACTIVE_CAMERA_STATUS -> {
                   val cameraActive = msg.data.getBoolean("cameraActive")
                   if (cameraActive) {
                       // Camera is active - try to reconnect
                       val receivedSharedMemoryPtr = msg.data.getLong("sharedMemoryPtr")
                       @Suppress("DEPRECATION")
                       val parcelFd = msg.data.getParcelable<ParcelFileDescriptor>("sharedMemoryFd")
                       val receivedCameraWidth = msg.data.getInt("cameraWidth")
                       val receivedCameraHeight = msg.data.getInt("cameraHeight")
                       val cameraIndex = msg.data.getInt("cameraIndex")
                       
                       if (parcelFd != null) {
                           Log.i("UserInterface", "Active camera found (index $cameraIndex) - reconnecting")
                           selectedCameraIndex = cameraIndex
                           onCameraOpened(receivedSharedMemoryPtr, parcelFd.fd, receivedCameraWidth, receivedCameraHeight)
                       } else {
                           // Failed to get SharedMemory - show menu
                           requestCameraEnumeration()
                       }
                   } else {
                       // No active camera - show menu
                       requestCameraEnumeration()
                   }
               }
               CameraInterface.MSG_CAMERA_OPENED -> {
                   val success = msg.data.getBoolean("success")
                   if (success) {
                       val receivedSharedMemoryPtr = msg.data.getLong("sharedMemoryPtr")
                       @Suppress("DEPRECATION")
                       val parcelFd = msg.data.getParcelable<ParcelFileDescriptor>("sharedMemoryFd")
                       val receivedCameraWidth = msg.data.getInt("cameraWidth")
                       val receivedCameraHeight = msg.data.getInt("cameraHeight")
                       
                       if (parcelFd != null) {
                           val receivedSharedMemoryFd = parcelFd.fd
                           Log.i("UserInterface", "Camera opened successfully - switching to camera UI")
                           onCameraOpened(receivedSharedMemoryPtr, receivedSharedMemoryFd, receivedCameraWidth, receivedCameraHeight)
                       } else {
                           Log.e("UserInterface", "ParcelFileDescriptor is null - kill switch activated")
                           moveTaskToBack(true)
                           Process.killProcess(Process.myPid())
                       }
                   } else {
                       Log.e("UserInterface", "Camera failed to open - kill switch activated")
                       moveTaskToBack(true)
                       Process.killProcess(Process.myPid())
                   }
               }
           }
       }
   })
   
   // Service connection for CameraInterface
   private val serviceConnection = object : ServiceConnection {
       override fun onServiceConnected(className: ComponentName, service: IBinder) {
           cameraService = Messenger(service)
           serviceBound = true
           Log.i("UserInterface", "Connected to CameraInterface")
           
           // Check if camera is already active first
           val msg = Message.obtain(null, CameraInterface.MSG_CHECK_ACTIVE_CAMERA)
           msg.replyTo = clientMessenger
           
           try {
               cameraService?.send(msg)
           } catch (e: RemoteException) {
               Log.e("UserInterface", "Failed to send camera enumeration request: $e")
           }
       }
       
       override fun onServiceDisconnected(className: ComponentName) {
           serviceBound = false
           Log.e("UserInterface", "CameraInterface disconnected - kill switch activated")
           moveTaskToBack(true)
       Process.killProcess(Process.myPid())
       }
   }

   // Gravity sensor listener - sends raw values to Rust
   private val gravityListener = object : SensorEventListener {
       override fun onSensorChanged(event: SensorEvent) {
           if (event.sensor.type == Sensor.TYPE_GRAVITY) {
               gravityX = event.values[0]
               gravityY = event.values[1]  
               gravityZ = event.values[2]
               // Rust will handle rotation calculations
           }
       }
       
       override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}
   }
   
   companion object {
       init { System.loadLibrary("lumis_core") }
       
       // SharedMemory flag constants
       const val COMPLETE_EXPOSURE_FLAG = 0x01L
       
       @JvmStatic
       var instance: UserInterface? = null
   }
   
   // Menu JNI functions
   external fun nativeMenuInit(width: Int, height: Int, density: Int, cameraArray: FloatArray): Long
   external fun nativeMenuHandleTouch(contextPtr: Long, action: Int, x: Float, y: Float): IntArray
   external fun nativeMenuDraw(contextPtr: Long, surface: Surface, fullDraw: Boolean)
   
   // UI JNI functions
   external fun nativeSetConfigDir(configDir: String)
   external fun nativeSetMachineId(machineId: String)
   external fun nativeUIInit(sharedMemoryFd: Int, sharedMemorySize: Long, surface: Surface, width: Int, height: Int, density: Int): Long
   external fun nativeUIDraw(
       contextPtr: Long, 
       surface: Surface,
       completeExposure: Boolean,
       save: Boolean,
       continuousSave: Boolean,
       gravityX: Float,         // Raw gravity values
       gravityY: Float,
       gravityZ: Float,
       touchX: Float,           // NaN if no touch
       touchY: Float
   )
   
   override fun onCreate(savedInstanceState: Bundle?) {
       Log.e("UserInterface", "onCreate() called - ENTRY POINT")
       super.onCreate(savedInstanceState)
       Log.e("UserInterface", "super.onCreate() completed")

       instance = this
       window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

       // Give SurfaceFlinger a display-native, wide-gamut target so it doesn't clamp our BT.2020-tagged preview buffer toward sRGB or run a vendor saturation pass. The Rust side tags the surface dataspace BT.2020 + gamma2.2; this is the Activity-side half. preferMinimalPostProcessing asks the panel to skip its own colour processing.
       if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
           window.attributes = window.attributes.apply {
               colorMode = android.content.pm.ActivityInfo.COLOR_MODE_WIDE_COLOR_GAMUT
               if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                   preferMinimalPostProcessing = true
               }
           }
       }

       // Set the config directory for Rust calibration code
       nativeSetConfigDir(filesDir.absolutePath)
       // Provide ANDROID_ID for calibration-file device binding (machine_uid has no Android backend; the Rust/chameleon side uses this instead).
       val androidId = android.provider.Settings.Secure.getString(
           contentResolver, android.provider.Settings.Secure.ANDROID_ID
       ) ?: ""
       nativeSetMachineId(androidId)
       
       Log.i("UserInterface", "Starting in MENU state")
       currentState = UIState.MENU
       
       // Create surface view for both menu and camera UI
       surfaceView = SurfaceView(this)
       surfaceView.holder.addCallback(this)
       surfaceView.holder.setFormat(PixelFormat.RGB_888)
       setContentView(surfaceView)
       
       // Setup fullscreen after content view is set
       setupFullscreen()
       
       // Initialize sensors
       sensorManager = getSystemService(Context.SENSOR_SERVICE) as SensorManager
       gravitySensor = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY)
       
       // Initialize power management  
       powerManager = getSystemService(Context.POWER_SERVICE) as PowerManager
       wakeLock = powerManager.newWakeLock(
           PowerManager.PARTIAL_WAKE_LOCK or PowerManager.ON_AFTER_RELEASE,
           "Lumis::CameraWakeLock"
       ).apply {
           setReferenceCounted(false)
       }
       
       // Initialize auto-nuke handler
       autoNukeHandler = Handler(Looper.getMainLooper())
       
       // Bind to the camera service. BIND_AUTO_CREATE creates it, which runs its onCreate() -> startForeground(), so it survives for the life of the binding.
       // Note: we must NOT call startService() here. On Android 12+ a plain startService() from an activity that was launched by a finishing launcher activity counts as a "background service start" and throws BackgroundServiceStartNotAllowedException, crashing onCreate. Binding is not subject to that restriction and gives us the same service lifetime.
       bindToCameraService()
   }

   private fun bindToCameraService() {
       val intent = Intent(this, CameraInterface::class.java)
       bindService(intent, serviceConnection, Context.BIND_AUTO_CREATE)
   }
   
   
   private fun requestCameraEnumeration() {
       Log.i("UserInterface", "Requesting camera enumeration for menu")
       val msg = Message.obtain(null, CameraInterface.MSG_ENUMERATE_CAMERAS)
       msg.replyTo = clientMessenger
       
       try {
           cameraService?.send(msg)
       } catch (e: RemoteException) {
           Log.e("UserInterface", "Failed to send camera enumeration request: $e")
       }
   }
   
   override fun surfaceCreated(holder: SurfaceHolder) {
       surfaceWidth = holder.surfaceFrame.width()
       surfaceHeight = holder.surfaceFrame.height()
       surfaceDensity = resources.displayMetrics.densityDpi
       
       Log.i("UserInterface", "Surface created - waiting for camera enumeration")
       
       // If cameras already enumerated (service connected fast), create menu now
       if (cameraArray != null && menuContextPtr == 0L) {
           menuContextPtr = nativeMenuInit(surfaceWidth, surfaceHeight, surfaceDensity, cameraArray!!)
           if (menuContextPtr != 0L) {
               Log.i("UserInterface", "Menu initialized immediately with cameras")
               drawFrame() // Initial menu draw
           }
       }
       // Otherwise wait for onCamerasEnumerated
   }

   override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
       // Surface changes handled by native code
   }

   override fun surfaceDestroyed(holder: SurfaceHolder) {
       renderLoopActive = false
   }
   
   private fun onCamerasEnumerated(receivedCameraArray: FloatArray) {
       cameraArray = receivedCameraArray
       
       // If surface is ready but menu not created yet, create it now
       if (menuContextPtr == 0L && surfaceWidth > 0) {
           menuContextPtr = nativeMenuInit(surfaceWidth, surfaceHeight, surfaceDensity, cameraArray!!)
           if (menuContextPtr != 0L) {
               Log.i("UserInterface", "Menu initialized with ${cameraArray!!.size} camera elements")
               drawFrame() // Initial menu draw
           }
       }
   }
   
   private fun onCameraOpened(receivedSharedMemoryPtr: Long, receivedSharedMemoryFd: Int, receivedCameraWidth: Int, receivedCameraHeight: Int) {
       sharedMemoryPtr = receivedSharedMemoryPtr
       sharedMemoryFd = receivedSharedMemoryFd
       cameraWidth = receivedCameraWidth
       cameraHeight = receivedCameraHeight
       
       Log.i("UserInterface", "Camera opened - switching to CAMERA_READY state")
       currentState = UIState.CAMERA_READY
       
       onCameraReady()
   }
   
   private fun onCameraReady() {
       Log.i("UserInterface", "onCameraReady called with SharedMemory fd=$sharedMemoryFd, camera=${cameraWidth}x${cameraHeight}")
       
       // Initialize UI JNI with SharedMemory file descriptor
       val width = surfaceView.holder.surfaceFrame.width()
       val height = surfaceView.holder.surfaceFrame.height()
       val density = resources.displayMetrics.densityDpi
       
       // Calculate SharedMemory size - SAME calculation as camera integrator
       val pixelCount = cameraWidth * cameraHeight
       val rawBufferSizeBytes = pixelCount * 16  // 2 u16 arrays, 4 bytes per pixel, quad rolling buffer
       val headerSizeBytes = 64 * 8  // IMAGE_START (64 u64s) * 8 bytes - must match jni_camera.rs
       val sharedMemorySize = (headerSizeBytes + rawBufferSizeBytes).toLong()
       
       Log.i("UserInterface", "UI dimensions: ${width}x${height} @ ${density}dpi, SharedMemory size: $sharedMemorySize")
       
       surfaceView.holder.surface?.let { surface ->
           Log.i("UserInterface", "Creating UI context...")
           uiContextPtr = nativeUIInit(sharedMemoryFd, sharedMemorySize, surface, width, height, density)
           if (uiContextPtr != 0L) {
               startRenderLoop() // Start continuous UI rendering for heartbeats
               Log.i("UserInterface", "UI context initialized - entering camera mode")
           } else {
               Log.e("UserInterface", "Failed to initialize UI context")
           }
       } ?: run {
           Log.e("UserInterface", "Surface is null - cannot initialize UI")
       }
   }
   
   private fun drawUIFrame() {
       if (uiContextPtr != 0L) {
           surfaceView.holder.surface?.let { surface ->
               // Snapshot the touch once so onTouchEvent (UI thread) can't mutate it mid-call. If this frame carries a valid touch, mark the current down as delivered to a draw - a later UP can now clear immediately rather than deferring.
               val touchX = currentTouchX
               val touchY = currentTouchY
               if (!touchX.isNaN()) {
                   touchDownSampled = true
               }
               nativeUIDraw(
                   uiContextPtr,
                   surface,
                   bluetoothShutterPressed,  // completeExposure - Bluetooth shutter
                   savePressed,              // save - volume up
                   continuousSavePressed,    // continuousSave - volume down
                   gravityX,                 // Raw gravity for Rust rotation handling
                   gravityY,
                   gravityZ,
                   touchX,                   // NaN if no touch
                   touchY
               )
               // A tap whose UP arrived before its DOWN was sampled deferred the NaN clear (pendingTouchUp). We've now delivered that DOWN to a draw, so release the touch: next frame sends NaN, giving Rust the DOWN -> UP transition it needs to register the tap.
               if (pendingTouchUp) {
                   currentTouchX = Float.NaN
                   currentTouchY = Float.NaN
                   pendingTouchUp = false
                   touchDownSampled = false
               }
               // Reset flags after sending
               if (bluetoothShutterPressed) {
                   bluetoothShutterPressed = false
               }
               if (savePressed) {
                   savePressed = false
               }
               if (continuousSavePressed) {
                   continuousSavePressed = false
               }
           }
       }
   }
   
   private fun startRenderLoop() {
       if (!renderLoopActive) {
           renderLoopActive = true
           Choreographer.getInstance().postFrameCallback(frameCallback)
           Log.i("UserInterface", "UI render loop started")
       }
   }
   
   private fun setupFullscreen() {
       if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
           window.setDecorFitsSystemWindows(false)
           window.insetsController?.let {
               it.hide(WindowInsets.Type.statusBars() or WindowInsets.Type.navigationBars())
               it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
           }
           window.attributes.layoutInDisplayCutoutMode = 
               WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
       } else {
           @Suppress("DEPRECATION")
           window.decorView.systemUiVisibility = (
               View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
               View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
               View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
               View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
               View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
               View.SYSTEM_UI_FLAG_FULLSCREEN
           )
           window.addFlags(WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS)
           
           if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
               window.attributes.layoutInDisplayCutoutMode = 
                   WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
           }
       }
   }
   
   override fun onWindowFocusChanged(hasFocus: Boolean) {
       super.onWindowFocusChanged(hasFocus)
       if (hasFocus) {
           setupFullscreen()
       }
   }

    override fun onResume() {
        super.onResume()
        
        // Cancel auto-nuke timer if user returns quickly
        autoNukeHandler?.removeCallbacks(autoNukeRunnable)
        Log.i("UserInterface", "Focus regained - auto-nuke timer cancelled")
        
        gravitySensor?.let {
            sensorManager.registerListener(gravityListener, it, SensorManager.SENSOR_DELAY_UI)
        }
        
        when (currentState) {
            UIState.MENU -> {
                // Redraw menu after surface change
                if (menuContextPtr != 0L) {
                    drawFrame()
                }
            }
            UIState.CAMERA_READY -> {
                // Restart UI render loop after surface change
                if (uiContextPtr != 0L) {
                    startRenderLoop()
                }
            }
            UIState.CAMERA_OPENING -> {
                // Don't draw anything - keep frozen
            }
        }
    }

    override fun onPause() {
        super.onPause()
        sensorManager.unregisterListener(gravityListener)
        renderLoopActive = false  // Stop render loop - UI will auto-nuke after 2 seconds
        
        // Start 2-second auto-nuke timer
        autoNukeHandler?.postDelayed(autoNukeRunnable, 2000)
        Log.i("UserInterface", "Focus lost - auto-nuke timer started (2 seconds)")
    }

    override fun onDestroy() {
        super.onDestroy()
        if (serviceBound) {
            unbindService(serviceConnection)
            serviceBound = false
        }
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        // Don't restart - just continue
    }
   
   override fun onTouchEvent(event: MotionEvent): Boolean {
       when (currentState) {
           UIState.MENU -> {
               // Handle menu touches
               if (menuContextPtr != 0L) {
                   val result = nativeMenuHandleTouch(menuContextPtr, event.actionMasked, event.x, event.y)
                   
                   if (result.size >= 2) {
                       val needsRedraw = result[0] != 0
                       val cameraSelected = result[1]
                       
                       // Only draw if button state changed
                       if (needsRedraw) {
                           drawFrame() // Redraw menu
                       }
                       
                       if (cameraSelected >= 0) {
                           // Camera selected - freeze menu and open camera
                           selectedCameraIndex = cameraSelected
                           currentState = UIState.CAMERA_OPENING
                           Log.i("UserInterface", "Camera $cameraSelected selected - opening camera")
                           openCamera(cameraSelected)
                       }
                   }
               }
           }
           UIState.CAMERA_OPENING -> {
               // Ignore touches while camera is opening
           }
           UIState.CAMERA_READY -> {
               // Handle camera UI touches
               when (event.actionMasked) {
                   MotionEvent.ACTION_DOWN, MotionEvent.ACTION_MOVE -> {
                       currentTouchX = event.x
                       currentTouchY = event.y
                       if (event.actionMasked == MotionEvent.ACTION_DOWN) {
                           // Fresh press: it has not been handed to a draw yet, and any deferred release from a prior tap is moot.
                           touchDownSampled = false
                           pendingTouchUp = false
                       }
                   }
                   else -> {
                       // ANY other action (UP, CANCEL, OUTSIDE, POINTER_DOWN, etc.). Clear the coordinates - but if the matching DOWN was never sampled by a draw (a tap shorter than one slow frame), defer the clear one frame so the draw loop still sees the DOWN before the UP. Otherwise clear immediately.
                       if (touchDownSampled) {
                           currentTouchX = Float.NaN
                           currentTouchY = Float.NaN
                           pendingTouchUp = false
                       } else {
                           // Keep currentTouchX/Y at the down position for one more frame; the render loop will sample the DOWN, then honour this pending UP.
                           pendingTouchUp = true
                       }
                   }
               }
           }
       }
       
       return true
   }
   
   override fun onKeyDown(keyCode: Int, event: KeyEvent?): Boolean {
       if (event != null) {
           val deviceName = event.device?.name ?: ""
           // Diagnostic: which input device sent this key? The save trigger matches on gpio-keys/qpnp_pon; on some phones the volume keys report a different name.
           Log.i("Lumis", "onKeyDown keyCode=$keyCode device='$deviceName' source=${event.source}")

           // Handle volume keys from physical buttons for save functions
           // The on-device volume keypad reports "gpio_keys"/"gpio-keys" (varies by device, e.g. Pixel uses the underscore) or "qpnp_pon"; anything else (BT remotes like "AB Shutter3", keyboards) is treated as a shutter.
           val isPhysicalButton = deviceName.contains("gpio") || deviceName.contains("qpnp_pon")

           if ((keyCode == KeyEvent.KEYCODE_VOLUME_UP || keyCode == KeyEvent.KEYCODE_VOLUME_DOWN) &&
               isPhysicalButton) {
               // Physical volume buttons - handle save functions
               if (uiContextPtr != 0L) {
                   when (keyCode) {
                       KeyEvent.KEYCODE_VOLUME_UP -> {
                           // Volume up: single save or cancel continuous save
                           Log.i("Lumis", "Physical volume up - single save")
                           savePressed = true
                       }
                       KeyEvent.KEYCODE_VOLUME_DOWN -> {
                           // Volume down: start continuous save
                           Log.i("Lumis", "Physical volume down - continuous save")
                           continuousSavePressed = true
                       }
                   }
                   return true // Consume the event
               }
           }
           
           // Handle Bluetooth shutter buttons (various keycodes from non-physical devices)
           if (!isPhysicalButton) {
               when (keyCode) {
                   KeyEvent.KEYCODE_VOLUME_UP,
                   KeyEvent.KEYCODE_VOLUME_DOWN,
                   KeyEvent.KEYCODE_ENTER,
                   KeyEvent.KEYCODE_SPACE -> {
                       Log.i("Lumis", "Bluetooth shutter pressed (keycode: $keyCode) - resetting exposure timer")
                       if (uiContextPtr != 0L) {
                           // Trigger exposure completion on next frame
                           bluetoothShutterPressed = true
                       }
                       return true // Consume the event
                   }
               }
           }
       }
       
       return super.onKeyDown(keyCode, event)
   }
   
   private fun openCamera(cameraIndex: Int): Boolean {
       Log.i("UserInterface", "openCamera() called with cameraIndex: $cameraIndex")
       
       if (!serviceBound) {
           Log.e("UserInterface", "Cannot open camera - service not bound")
           return false
       }
       
       return sendCameraOpenRequest(cameraIndex)
   }
   
   private fun sendCameraOpenRequest(cameraIndex: Int): Boolean {
       val msg = Message.obtain(null, CameraInterface.MSG_OPEN_CAMERA)
       msg.data = Bundle().apply {
           putInt("cameraIndex", cameraIndex)
       }
       msg.replyTo = clientMessenger
       
       try {
           cameraService?.send(msg)
           Log.i("UserInterface", "Sent MSG_OPEN_CAMERA to CameraInterface for camera $cameraIndex")
           return true
       } catch (e: RemoteException) {
           Log.e("UserInterface", "Failed to send camera open message: $e")
           return false
       }
   }
   
   private fun drawFrame() {
       when (currentState) {
           UIState.MENU -> {
               // Draw menu
               if (menuContextPtr != 0L) {
                   surfaceView.holder.surface?.let { surface ->
                       nativeMenuDraw(menuContextPtr, surface, false)
                   }
               }
           }
           UIState.CAMERA_OPENING -> {
               // Don't draw anything - keep frozen menu on screen
           }
           UIState.CAMERA_READY -> {
               // Draw camera UI (handled by continuous render loop)
           }
       }
   }
}