package com.lumis.camera

import android.app.Service
import android.content.Context
import android.content.Intent
import android.graphics.ImageFormat
import android.hardware.camera2.*
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.ImageReader
import android.os.*
import android.util.Log
import android.util.Size
import android.annotation.SuppressLint
import androidx.core.content.ContextCompat
import java.nio.ByteBuffer
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import androidx.core.app.NotificationCompat
import android.content.pm.ServiceInfo
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.content.ContentValues
import android.content.ContentUris
import java.io.IOException
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager

class CameraInterface : Service() {
   
   // Native context pointer for Camera processing ONLY
   internal var nativeCameraContextPtr: Long = 0L
   
   // Camera processor instance
   private lateinit var cameraProcessor: CameraProcessor
   
   // Messenger for IPC with Menu and UserInterface
   private lateinit var serviceMessenger: Messenger
   
   // Audio management for STREAM_VOICE_CALL
   private lateinit var audioManager: AudioManager
   private var audioFocusRequest: AudioFocusRequest? = null
   private var previousStreamType: Int? = null
   
   // Active camera state tracking
   private var activeCameraIndex: Int = -1
   private var isCameraActive: Boolean = false
   private var isContinuousSaveActive: Boolean = false
   
   // Handler for incoming messages from Menu and UserInterface
   private val messageHandler = object : Handler(Looper.getMainLooper()) {
       override fun handleMessage(msg: Message) {
           when (msg.what) {
               MSG_ENUMERATE_CAMERAS -> {
                   val cameraArray = enumerateRawCameras()
                   val replyMsg = Message.obtain(null, MSG_CAMERAS_ENUMERATED)
                   val replyData = Bundle()
                   replyData.putFloatArray("cameras", cameraArray)
                   replyMsg.data = replyData
                   
                   msg.replyTo!!.send(replyMsg)
               }
               
               MSG_OPEN_CAMERA -> {
                   val cameraIndex = msg.data.getInt("cameraIndex")
                   
                   cameraProcessor.setReplyMessenger(msg.replyTo)
                   openCamera(cameraIndex)
               }
               
               MSG_CHECK_ACTIVE_CAMERA -> {
                   val replyMsg = Message.obtain(null, MSG_ACTIVE_CAMERA_STATUS)
                   val replyData = Bundle()
                   
                   if (isCameraActive && nativeCameraContextPtr != 0L) {
                       // Camera is active - send SharedMemory info
                       replyData.putBoolean("cameraActive", true)
                       replyData.putInt("cameraIndex", activeCameraIndex)
                       replyData.putLong("sharedMemoryPtr", nativeCameraGetSharedMemoryPtr(nativeCameraContextPtr))
                       
                       // Wrap fd in ParcelFileDescriptor for IPC
                       val sharedMemoryFd = nativeCameraGetSharedMemoryFd(nativeCameraContextPtr)
                       try {
                           val parcelFd = ParcelFileDescriptor.fromFd(sharedMemoryFd)
                           replyData.putParcelable("sharedMemoryFd", parcelFd)
                       } catch (e: Exception) {
                           Log.e("CameraInterface", "Failed to create ParcelFileDescriptor: $e")
                           replyData.putBoolean("cameraActive", false)
                       }
                       
                       replyData.putInt("cameraWidth", nativeCameraGetWidth(nativeCameraContextPtr))
                       replyData.putInt("cameraHeight", nativeCameraGetHeight(nativeCameraContextPtr))
                   } else {
                       // No active camera
                       replyData.putBoolean("cameraActive", false)
                   }
                   
                   replyMsg.data = replyData
                   msg.replyTo!!.send(replyMsg)
               }
           }
       }
   }
   
   companion object {
       // Message types for IPC
       const val MSG_ENUMERATE_CAMERAS = 1
       const val MSG_CAMERAS_ENUMERATED = 2
       const val MSG_OPEN_CAMERA = 3
       const val MSG_CAMERA_OPENED = 4
       const val MSG_CLOSE_CAMERA = 5
       const val MSG_CHECK_ACTIVE_CAMERA = 6
       const val MSG_ACTIVE_CAMERA_STATUS = 7
       
       // Notification constants
       private const val NOTIFICATION_ID = 1001
       private const val CHANNEL_ID = "lumis_camera_service"
       
       // JNI functions for Camera processing
       @JvmStatic
       external fun nativeCameraInit(
           width: Int, 
           height: Int,
           whiteLevel: Int,
           blackLevel: Int,
           bayerPattern: Int,
           cameraFacing: Int,
           sensorOrientation: Int,
           minIso: Int,
           maxIso: Int,
           minExposure: Long,
           maxExposure: Long,
           minFocus: Float,
           initialIso: Int,
           initialShutterNs: Long
       ): Long
       
       @JvmStatic
       external fun nativeCameraOnFrame(
           contextPtr: Long,
           buffer: ByteBuffer,
           capturedIso: Int,
           capturedShutterNs: Long,
           capturedFocusDistance: Float,
           raw10: Boolean,
           rowStride: Int
       ): CameraSettings
       
       @JvmStatic
       external fun nativeGetCurrentSettings(contextPtr: Long): CameraSettings

       @JvmStatic
       external fun nativeCameraGetSharedMemoryPtr(contextPtr: Long): Long
       
       @JvmStatic
       external fun nativeCameraGetSharedMemoryFd(contextPtr: Long): Int
       
       @JvmStatic
       external fun nativeCameraGetWidth(contextPtr: Long): Int
       
       @JvmStatic
       external fun nativeCameraGetHeight(contextPtr: Long): Int
       
       // Static instance reference for JNI access
       @JvmStatic
       var instance: CameraInterface? = null
       
       // Static method called from JNI to save DNG data
       @JvmStatic
       fun saveDngData(dngData: ByteArray, filename: String): Boolean {
           return instance?.saveImageToMediaStoreImpl(dngData, filename, "image/x-adobe-dng") ?: false
       }

       init {
           System.loadLibrary("lumis_core")
       }
   }
   
   // Native methods
   external fun nativeGetSavedDngData(): SaveDng?
   external fun nativeClearSaveInProgress(ptr: Long)

   // Map the Rust-assigned file extension to a MediaStore mime type.
   fun mimeForFilename(filename: String): String = when (filename.substringAfterLast('.').lowercase()) {
       "dng" -> "image/x-adobe-dng"
       "tiff", "tif" -> "image/tiff"
       "jxl" -> "image/jxl"
       else -> "image/jpeg"
   }

   // Called from JNI to save image data using MediaStore
   fun saveImageToMediaStoreImpl(imageData: ByteArray, filename: String, mimeType: String): Boolean {
       Log.i("CameraInterface", "Attempting to save: $filename")
       
       // Check if file already exists
       val projection = arrayOf(MediaStore.Images.Media._ID, MediaStore.Images.Media.DISPLAY_NAME)
       val selection = "${MediaStore.Images.Media.DISPLAY_NAME} = ?"
       val selectionArgs = arrayOf(filename)
       
       contentResolver.query(
           MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
           projection,
           selection,
           selectionArgs,
           null
       )?.use { cursor ->
           if (cursor.moveToFirst()) {
               val existingName = cursor.getString(cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DISPLAY_NAME))
               Log.i("CameraInterface", "File already exists: '$existingName' - skipping save to prevent duplicate")
               return true // File exists, don't save again
           } else {
               Log.i("CameraInterface", "No existing file found with name: $filename - proceeding with save")
           }
       }
       
       val values = ContentValues().apply {
           put(MediaStore.Images.Media.DISPLAY_NAME, filename)
           put(MediaStore.Images.Media.MIME_TYPE, mimeType)
           if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
               put(MediaStore.Images.Media.RELATIVE_PATH, "Pictures/Lumis")
               put(MediaStore.Images.Media.IS_PENDING, 1)
           }
       }
       
       if (imageData.isEmpty()) {
           Log.e("CameraInterface", "Refusing to save '$filename': image data is empty (0 bytes)")
           return false
       }
       return try {
           val uri = contentResolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)
           if (uri != null) {
               var written = 0L
               contentResolver.openOutputStream(uri)?.use { stream ->
                   stream.write(imageData)
                   stream.flush()
                   written = imageData.size.toLong()
               }
               if (written == 0L) {
                   // openOutputStream returned null or wrote nothing: the row would be a dangling
                   // 0-byte entry that the media scanner reaps. Delete it and report failure.
                   Log.e("CameraInterface", "0 bytes written for '$filename' - deleting dangling entry")
                   contentResolver.delete(uri, null, null)
                   return false
               }

               // Mark as complete (Android 10+)
               if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                   values.clear()
                   values.put(MediaStore.Images.Media.IS_PENDING, 0)
                   contentResolver.update(uri, values, null, null)
               }

               // Get the actual filename that was created
               val actualName = contentResolver.query(uri, arrayOf(MediaStore.Images.Media.DISPLAY_NAME), null, null, null)?.use { cursor ->
                   if (cursor.moveToFirst()) cursor.getString(0) else null
               }
               Log.i("CameraInterface", "Saved image to MediaStore - requested: '$filename', actual: '$actualName', bytes: $written, uri: $uri")
               true
           } else {
               Log.e("CameraInterface", "Failed to create MediaStore entry for $filename (insert returned null)")
               false
           }
       } catch (e: Exception) {
           // Broadened from IOException: a bad DISPLAY_NAME (illegal chars) or unsupported MIME throws
           // IllegalArgumentException from insert(), which previously escaped uncaught and lost the save.
           Log.e("CameraInterface", "Failed to save image '$filename': ${e.javaClass.simpleName}: ${e.message}")
           false
       }
   }
   
   override fun onCreate() {
       super.onCreate()
       Log.i("CameraInterface", "CameraInterface created")
       
       // Set static instance for JNI access
       instance = this
       
       serviceMessenger = Messenger(messageHandler)
       cameraProcessor = CameraProcessor(this)
       
       // Initialize audio manager
       audioManager = getSystemService(Context.AUDIO_SERVICE) as AudioManager
       
       // Start as foreground service to survive activity destruction
       if (Build.VERSION.SDK_INT >= 34) {
           startForeground(
               NOTIFICATION_ID, 
               createNotification(),
               ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA
           )
       } else {
           startForeground(NOTIFICATION_ID, createNotification())
       }
   }
   
   
   override fun onBind(intent: Intent?): IBinder {
       Log.i("CameraInterface", "Activity binding to CameraInterface")
       return serviceMessenger.binder
   }
   
   override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
       return START_STICKY
   }
   
   private fun createNotificationChannel() {
       if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
           val channel = NotificationChannel(
               CHANNEL_ID,
               "Camera Processing Service",
               NotificationManager.IMPORTANCE_LOW
           ).apply {
               description = "Maintains camera processing across app restarts"
               setShowBadge(false)
           }
           
           val notificationManager = getSystemService(NotificationManager::class.java)
           notificationManager.createNotificationChannel(channel)
       }
   }
   
   private fun createNotification(): Notification {
       createNotificationChannel()
       
       // Create intent for UserInterface
       val intent = Intent(this, UserInterface::class.java)
       val pendingIntent = PendingIntent.getActivity(
           this,
           0,
           intent,
           PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
       )
       
       return NotificationCompat.Builder(this, CHANNEL_ID)
           .setContentTitle("Lumis Camera")
           .setContentText("Auto-capture active")
           .setSmallIcon(android.R.drawable.ic_menu_camera)
           .setContentIntent(pendingIntent)
           .setOngoing(true)
           .setPriority(NotificationCompat.PRIORITY_LOW)
           .build()
   }
   
   private var activeCameraInfo: CameraInfo? = null

   private fun openCamera(cameraIndex: Int) {
       val cameraInfo = getCameraInfo(cameraIndex)!!

       // Track active camera
       activeCameraIndex = cameraIndex
       activeCameraInfo = cameraInfo
       isCameraActive = true

       // Configure audio for STREAM_VOICE_CALL before starting camera
       configureAudioForCamera()

       // The integrator is NOT created here. CameraProcessor first runs a one-shot auto-exposure to meter the scene, then calls back to initIntegrator() with the metered ISO/shutter so the manual sliders open at usable values on any device. After that the camera is fully manual.
       cameraProcessor.openCamera(cameraInfo.logicalId, cameraInfo.physicalId, cameraInfo.maxRes)
   }

   // Called by CameraProcessor once one-shot AE has settled, with the metered values.
   internal fun initIntegrator(aeIso: Int, aeShutterNs: Long) {
       val cameraInfo = activeCameraInfo ?: return
       nativeCameraContextPtr = nativeCameraInit(
           cameraInfo.widthPixels,
           cameraInfo.heightPixels,
           cameraInfo.whiteLevel,
           cameraInfo.blackLevel,
           cameraInfo.bayerPattern,
           cameraInfo.facing,
           cameraInfo.sensorOrientation,
           cameraInfo.minIso,
           cameraInfo.maxIso,
           cameraInfo.minExposure,
           cameraInfo.maxExposure,
           cameraInfo.minFocusDistance,
           aeIso,
           aeShutterNs
       )
       cameraProcessor.setNativeContext(nativeCameraContextPtr)
   }
   
   private fun getCameraInfo(cameraIndex: Int): CameraInfo? {
       val cameraArray = enumerateRawCameras()
       return parseSpecificCamera(cameraArray, cameraIndex)
   }
   
   private fun parseSpecificCamera(cameraArray: FloatArray, targetIndex: Int): CameraInfo? {
       // Parse camera array to find specific camera info
       var i = 0
       while (i < cameraArray.size) {
           if (cameraArray[i].isNaN()) {
               // Found camera header
               i++ // Skip NaN
               val index = cameraArray[i++].toInt()
               val idLength = cameraArray[i++].toInt()
               i += idLength // Skip camera ID string
               val facing = cameraArray[i++].toInt()
               val widthPixels = cameraArray[i++].toInt()
               val heightPixels = cameraArray[i++].toInt()
               val whiteLevel = cameraArray[i++].toInt()
               val blackLevel = cameraArray[i++].toInt()
               val bayerPattern = cameraArray[i++].toInt()
               i++ // Skip supportsRaw
               val minIso = cameraArray[i++].toInt()
               val maxIso = cameraArray[i++].toInt()
               val minExposure = cameraArray[i++].toLong()
               val maxExposure = cameraArray[i++].toLong()
               i++ // Skip sensorWidth (physical mm)
               i++ // Skip sensorHeight (physical mm)
               val focalLengthCount = cameraArray[i++].toInt()
               i += focalLengthCount // Skip focal lengths
               val apertureCount = cameraArray[i++].toInt()
               i += apertureCount // Skip apertures
               val minFocusDistance = cameraArray[i++]
               i++ // Skip hasOIS
               val hardwareLevel = cameraArray[i++].toInt()
               val sensorOrientation = cameraArray[i++].toInt()
               i++ // Skip pixelArrayWidth
               i++ // Skip modeCount
               i++ // Skip groupId
               val maxRes = cameraArray[i++] != 0f  // max-res (non-binned) readout
               i++ // Skip isCropped (display-only; carried in CameraData/Rust)
               i++ // Skip hasPhysical flag (+/-Inf)
               // Read logical id string
               val logicalIdLength = cameraArray[i++].toInt()
               val logicalIdSb = StringBuilder(logicalIdLength)
               for (c in 0 until logicalIdLength) { logicalIdSb.append(cameraArray[i++].toInt().toChar()) }
               // Read physical id string ("" means none)
               val physicalIdLength = cameraArray[i++].toInt()
               val physicalIdSb = StringBuilder(physicalIdLength)
               for (c in 0 until physicalIdLength) { physicalIdSb.append(cameraArray[i++].toInt().toChar()) }

               if (index == targetIndex) {
                   return CameraInfo(
                       widthPixels, heightPixels, whiteLevel, blackLevel, bayerPattern, facing,
                       sensorOrientation, minIso, maxIso, minExposure, maxExposure, minFocusDistance,
                       hardwareLevel,
                       logicalId = logicalIdSb.toString(),
                       physicalId = if (physicalIdSb.isEmpty()) null else physicalIdSb.toString(),
                       maxRes = maxRes
                   )
               }

               // Skip rest of this camera's data until the -0.0f terminator. Detect by sign bit: -0.0f == 0.0f by IEEE equality, so a plain "!= -0.0f" would also stop at a legitimate 0.0f field (e.g. groupId 0).
               while (i < cameraArray.size &&
                      !(cameraArray[i] == 0.0f && (1f / cameraArray[i]) < 0f)) {
                   i++
               }
               i++ // Skip -0.0f terminator
           } else {
               i++
           }
       }
       return null
   }
   
   internal fun onCameraInitializationComplete(replyTo: Messenger?) {
       val sharedMemoryPtr = nativeCameraGetSharedMemoryPtr(nativeCameraContextPtr)
       val sharedMemoryFd = nativeCameraGetSharedMemoryFd(nativeCameraContextPtr)
       val cameraWidth = nativeCameraGetWidth(nativeCameraContextPtr)
       val cameraHeight = nativeCameraGetHeight(nativeCameraContextPtr)
       
       val replyMsg = Message.obtain(null, MSG_CAMERA_OPENED)
       val replyData = Bundle()
       replyData.putBoolean("success", true)
       replyData.putLong("sharedMemoryPtr", sharedMemoryPtr)
       
       // Wrap the file descriptor in a ParcelFileDescriptor for proper IPC transfer
       try {
           val parcelFd = ParcelFileDescriptor.fromFd(sharedMemoryFd)
           replyData.putParcelable("sharedMemoryFd", parcelFd)
           Log.i("CameraInterface", "Created ParcelFileDescriptor from fd=$sharedMemoryFd")
       } catch (e: Exception) {
           Log.e("CameraInterface", "Failed to create ParcelFileDescriptor from fd=$sharedMemoryFd: $e")
           replyData.putBoolean("success", false)
       }
       
       replyData.putInt("cameraWidth", cameraWidth)
       replyData.putInt("cameraHeight", cameraHeight)
       replyMsg.data = replyData
       
       replyTo!!.send(replyMsg)
       Log.i("CameraInterface", "Camera initialization complete - SharedMemory ParcelFileDescriptor sent to UI")
   }
   
   override fun onDestroy() {
       Process.killProcess(Process.myPid())
   }
   
   private fun configureAudioForCamera() {
       // Request audio focus with STREAM_VOICE_CALL
       if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
           val audioAttributes = AudioAttributes.Builder()
               .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
               .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
               .build()
               
           audioFocusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
               .setAudioAttributes(audioAttributes)
               .setOnAudioFocusChangeListener { focusChange ->
                   Log.i("CameraInterface", "Audio focus change: $focusChange")
               }
               .build()
               
           val result = audioManager.requestAudioFocus(audioFocusRequest!!)
           Log.i("CameraInterface", "Audio focus request result: $result")
       } else {
           // Legacy audio focus request
           @Suppress("DEPRECATION")
           val result = audioManager.requestAudioFocus(
               { focusChange -> Log.i("CameraInterface", "Audio focus change: $focusChange") },
               AudioManager.STREAM_VOICE_CALL,
               AudioManager.AUDIOFOCUS_GAIN
           )
           Log.i("CameraInterface", "Legacy audio focus request result: $result")
       }
       
       // Set to STREAM_VOICE_CALL which continues playing after app is killed
       try {
           @Suppress("DEPRECATION")
           audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
           Log.i("CameraInterface", "Audio mode set to MODE_IN_COMMUNICATION for STREAM_VOICE_CALL")
       } catch (e: SecurityException) {
           Log.w("CameraInterface", "Cannot set audio mode: ${e.message}")
       }
   }
   
   @SuppressLint("MissingPermission")
   private fun enumerateRawCameras(): FloatArray {
       val cameraData = mutableListOf<Float>()
       
       try {
           val cameraManager = getSystemService(Context.CAMERA_SERVICE) as CameraManager
           val cameraIds = cameraManager.cameraIdList
           if (cameraIds.isEmpty()) {
               Log.w("CameraInterface", "No cameras found")
               return FloatArray(0)
           }
           
           val rawCameras = mutableListOf<CameraData>()
           val nonRawCameras = mutableListOf<CameraData>()

           // Monotonic index across BOTH top-level and hidden physical cameras, so every entry the menu shows has a stable, openable index.
           // We want ONE entry per real physical sensor. A logical multi-camera (Pixel rear/front) is a virtual wrapper that just fuses/switches among its physical sub-cameras, so listing both the logical AND its physicals produced dupes (the logical == its default physical). Rule: if a camera is a logical multi-camera with RAW physicals, list those physicals instead of itself; otherwise list the camera directly.
           var index = 0
           for (cameraId in cameraIds) {
               try {
                   val characteristics = cameraManager.getCameraCharacteristics(cameraId)
                   val physicalIds = getPhysicalIds(characteristics)

                   // Physical sub-cameras that advertise RAW, opened via this logical parent.
                   val rawPhysicals = mutableListOf<CameraData>()
                   for (physicalId in physicalIds) {
                       if (cameraIds.contains(physicalId)) continue // also a top-level id; handled there
                       try {
                           val physChars = cameraManager.getCameraCharacteristics(physicalId)
                           val physInfo = extractCameraData(
                               index, physicalId, physChars,
                               logicalId = cameraId, physicalId = physicalId
                           )
                           if (physInfo.supportsRaw) {
                               rawPhysicals.add(physInfo)
                               index++
                               // Offer a second entry for the non-binned max-res readout.
                               if (supportsMaxRes(physChars)) {
                                   rawPhysicals.add(extractCameraData(
                                       index, physicalId, physChars,
                                       logicalId = cameraId, physicalId = physicalId, maxRes = true
                                   ))
                                   index++
                               }
                           } else {
                               Log.i("CameraInterface", "Physical camera $physicalId (under $cameraId) has no RAW - skipping")
                           }
                       } catch (e: Exception) {
                           Log.e("CameraInterface", "Error processing physical camera $physicalId: ${e.message}")
                       }
                   }

                   if (rawPhysicals.isNotEmpty()) {
                       // Logical multi-camera: list its real sensors, NOT the logical wrapper. The physical sub-cameras expose their own RAW10 max-res config (added per-physical above), so we do NOT add a separate logical max-res entry - that duplicated the primary lens's 50MP mode.
                       rawCameras.addAll(rawPhysicals)
                   } else {
                       // Plain camera (or logical with no RAW physicals): list it directly.
                       val cameraInfo = extractCameraData(index++, cameraId, characteristics)
                       if (cameraInfo.supportsRaw) {
                           rawCameras.add(cameraInfo)
                           // Offer a second entry for the non-binned max-res readout.
                           if (supportsMaxRes(characteristics)) {
                               rawCameras.add(extractCameraData(
                                   index++, cameraId, characteristics, maxRes = true
                               ))
                           }
                       } else {
                           nonRawCameras.add(cameraInfo)
                       }
                   }
               } catch (e: Exception) {
                   Log.e("CameraInterface", "Error processing camera $cameraId: ${e.message}")
               }
           }
           
           // The HAL exposes the SAME physical lens several times as different capture modes (full-res vs binned, etc) - e.g. the Pixel main wide appears as two physical ids. Group by real lens, keyed by (facing, focal). We send ALL modes (not just a representative) so the menu can offer a second screen to pick a specific mode; the highest-res mode of each lens carries modeCount = group size (it's the row shown on the main screen and the group's head), and every mode in a lens shares a groupId so the menu can cluster them.
           // Group by real lens (facing + focal). ALL modes of a lens - binned, full-res (max-res), cropped - share one group so screen 1 shows one row per lens and the existing mode sub-screen (screen 2) lists that lens's resolution options.
           val rawByLens = rawCameras.groupBy { Pair(it.facing, lensFocalKey(it)) }

           // Order lenses: back before front (BACK=1 > FRONT=0), then ascending focal (wide -> tele). Within a lens, the HEAD (modeIdx 0, shown on screen 1 and opened by default) is the standard binned full-FOV mode - the safe/fast default - NOT the heavy max-res or cropped variants. Order: binned-full first, then the rest by descending resolution as drill-in options on screen 2.
           val modeRank = compareBy<CameraData>(
               { if (!it.maxRes && !it.isCropped) 0 else 1 }, // binned full-FOV first
               { -(it.width.toLong() * it.height.toLong()) }   // then largest first
           )
           val lensOrder = compareByDescending<List<CameraData>> { it.first().facing }
               .thenBy { it.first().focalLengths.firstOrNull() ?: Float.MAX_VALUE }
           val sortedLenses = rawByLens.values
               .map { group -> group.sortedWith(modeRank) }
               .sortedWith(lensOrder)

           val rawWithGroups = mutableListOf<CameraData>()
           for ((groupId, modes) in sortedLenses.withIndex()) {
               modes.forEachIndexed { modeIdx, mode ->
                   // Head mode (highest res, modeIdx 0) carries the real mode count so the menu shows "multiple" and treats it as the group head; others = 1.
                   rawWithGroups.add(mode.copy(
                       groupId = groupId,
                       modeCount = if (modeIdx == 0) modes.size else 1
                   ))
               }
           }

           val sortedNonRawCameras = nonRawCameras.sortedWith(
               compareByDescending<CameraData> { it.facing }
                   .thenBy { it.focalLengths.firstOrNull() ?: Float.MAX_VALUE }
           )

           // Reassign contiguous indices so the menu->open index mapping holds. Each mode keeps a unique index; StartCamera(index) opens that exact mode.
           val allCameras = (rawWithGroups + sortedNonRawCameras).mapIndexed { i, cam -> cam.copy(index = i) }

           for (camera in allCameras) {
               packCameraData(camera, cameraData)
           }

           Log.i("CameraInterface", "Enumerated ${sortedLenses.size} lenses, ${rawWithGroups.size} RAW modes total (${nonRawCameras.size} non-RAW)")
           
       } catch (e: SecurityException) {
           Log.e("CameraInterface", "Camera permission denied: ${e.message}")
           return FloatArray(0)
       } catch (e: Exception) {
           Log.e("CameraInterface", "Error enumerating cameras: ${e.message}")
           return FloatArray(0)
       }
       
       return cameraData.toFloatArray()
   }
   
   private data class CameraData(
       val index: Int,
       val androidCameraId: String,
       val cameraId: String,
       // The logical camera that must be opened to access this sensor. For a top-level camera this equals androidCameraId. For a hidden physical sub-camera (e.g. Pixel ultrawide/telephoto) this is the parent logical id.
       val logicalId: String,
       // Non-null only for hidden physical sub-cameras: the physical id to target via OutputConfiguration.setPhysicalCameraId().
       val physicalId: String?,
       val facing: Int,
       val width: Int,
       val height: Int,
       val whiteLevel: Int,
       val blackLevel: Int,
       val bayerPattern: Int,
       val supportsRaw: Boolean,
       val minIso: Int,
       val maxIso: Int,
       val minExposure: Long,
       val maxExposure: Long,
       val sensorWidth: Float,
       val sensorHeight: Float,
       val focalLengths: FloatArray,
       val apertures: FloatArray,
       val minFocusDistance: Float,
       val hasOis: Boolean,
       val hardwareLevel: Int,
       val sensorOrientation: Int,
       val pixelArrayWidth: Int,
       // Number of distinct capture modes this lens has, set on the group HEAD mode (>1 => menu shows "multiple" and opens the mode sub-picker). Non-head modes = 1.
       val modeCount: Int = 1,
       // Modes of the same physical lens share a groupId so the menu can cluster them and the sub-picker can list just that lens's modes. -1 = ungrouped.
       val groupId: Int = -1,
       // Maximum-resolution (non-binned) sensor readout. When true the stream uses the MaximumResolution stream config + SENSOR_PIXEL_MODE_MAXIMUM_RESOLUTION (e.g. the Pixel main lens's 50MP mode vs the default 12.5MP 2x2-binned mode).
       val maxRes: Boolean = false,
       // This RAW size images only a sub-region of the sensor's full active-array FOV (a cropped/digital-zoom readout) rather than the whole area.
       val isCropped: Boolean = false
   )

   // Group key identifying a real physical lens: facing + focal length (rounded to 0.1mm). The HAL lists one lens multiple times as different modes that share these.
   private fun lensFocalKey(camera: CameraData): Int {
       val f = if (camera.focalLengths.isNotEmpty()) camera.focalLengths[0] else 0f
       return Math.round(f * 10f)
   }

   private fun calculateViewingAngle(camera: CameraData): Float {
       if (camera.focalLengths.isEmpty() || camera.sensorWidth <= 0f || camera.sensorHeight <= 0f) {
           return Float.MAX_VALUE
       }
       
       val focalLength = camera.focalLengths[0]
       val sensorDiagonal = kotlin.math.sqrt(
           camera.sensorWidth * camera.sensorWidth + camera.sensorHeight * camera.sensorHeight
       )
       
       return Math.toDegrees(2.0 * kotlin.math.atan((sensorDiagonal / (2.0 * focalLength)).toDouble())).toFloat()
   }
   
   // Physical sub-camera ids behind a logical camera (API 28+). Empty otherwise.
   private fun getPhysicalIds(characteristics: CameraCharacteristics): Set<String> {
       if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) return emptySet()
       val caps = characteristics.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES) ?: intArrayOf()
       val isLogicalMulti = caps.contains(
           CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES_LOGICAL_MULTI_CAMERA
       )
       if (!isLogicalMulti) return emptySet()
       return try {
           characteristics.physicalCameraIds
       } catch (e: Exception) {
           Log.e("CameraInterface", "physicalCameraIds failed: ${e.message}")
           emptySet()
       }
   }

   private fun extractCameraData(
       index: Int,
       cameraId: String,
       characteristics: CameraCharacteristics,
       logicalId: String = cameraId,
       physicalId: String? = null,
       // When true, read sizes/levels from the MAXIMUM_RESOLUTION stream config (the non-binned native readout) instead of the default binned one.
       maxRes: Boolean = false
   ): CameraData {
       val capabilities = characteristics.get(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES) ?: intArrayOf()
       val supportsRaw = capabilities.contains(CameraCharacteristics.REQUEST_AVAILABLE_CAPABILITIES_RAW)

       val facing = characteristics.get(CameraCharacteristics.LENS_FACING) ?: CameraCharacteristics.LENS_FACING_BACK
       val sensorOrientation = characteristics.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 0
       val hardwareLevel = characteristics.get(CameraCharacteristics.INFO_SUPPORTED_HARDWARE_LEVEL) ?: 0

       // For max-res, the RAW sizes come from the MaximumResolution stream config map (API 31+); otherwise the default binned map.
       val streamConfigMap = if (maxRes && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
           characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP_MAXIMUM_RESOLUTION)
       } else {
           characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
       }
       // Max-res RAW comes from the high-resolution (stalling) size list; binned RAW from the regular one.
       val rawSizes = if (maxRes) {
           maxResRawSizes(streamConfigMap, characteristics)
       } else {
           (streamConfigMap?.getOutputSizes(ImageFormat.RAW_SENSOR) ?: arrayOf()).toList()
       }
       // Pick the largest RAW size offered (max-res maps may list several).
       val rawSize = rawSizes.maxByOrNull { it.width.toLong() * it.height.toLong() } ?: Size(0, 0)

       // Crop detection: does this RAW size cover the full active-array FOV, or only a sub-region? Compare its aspect ratio to the (max-res-aware) active array's. A mismatch beyond a small tolerance means a cropped/digital-zoom readout.
       val fullActive = if (maxRes && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
           characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE_MAXIMUM_RESOLUTION)
       } else {
           null
       } ?: characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)
       val isCropped = if (fullActive != null && rawSize.width > 0 && rawSize.height > 0
           && fullActive.width() > 0 && fullActive.height() > 0) {
           val rawAspect = rawSize.width.toFloat() / rawSize.height
           val activeAspect = fullActive.width().toFloat() / fullActive.height()
           kotlin.math.abs(rawAspect - activeAspect) > 0.02f
       } else {
           false
       }

       // SENSOR_INFO_PHYSICAL_SIZE covers the FULL sensor, but the lens only images onto the active (used) array - and FOV must be computed from the area that's actually imaged. Scale physical size by (activeArray / pixelArray) so the effective sensor dimensions match the projection. Without this the telephoto FOVs came out wrong (full-sensor diagonal overstated their angle).
       val physicalSize = characteristics.get(CameraCharacteristics.SENSOR_INFO_PHYSICAL_SIZE)
       val pixelArraySize = characteristics.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE)
       val activeArray = characteristics.get(CameraCharacteristics.SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE)
           ?: characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE)

       val physW = physicalSize?.width ?: 0f
       val physH = physicalSize?.height ?: 0f
       val pxW = pixelArraySize?.width ?: 0
       val pxH = pixelArraySize?.height ?: 0
       // Effective imaged dimensions (fall back to full physical size if arrays missing).
       val sensorWidth = if (activeArray != null && pxW > 0) physW * (activeArray.width() / pxW.toFloat()) else physW
       val sensorHeight = if (activeArray != null && pxH > 0) physH * (activeArray.height() / pxH.toFloat()) else physH

       val pixelArrayWidth = pxW

       val whiteLevel = characteristics.get(CameraCharacteristics.SENSOR_INFO_WHITE_LEVEL) ?: 1023
       val blackLevel = characteristics.get(CameraCharacteristics.SENSOR_BLACK_LEVEL_PATTERN)?.getOffsetForIndex(0, 0) ?: 64
       val bayerPattern = characteristics.get(CameraCharacteristics.SENSOR_INFO_COLOR_FILTER_ARRANGEMENT) ?: 0
       
       val isoRange = characteristics.get(CameraCharacteristics.SENSOR_INFO_SENSITIVITY_RANGE)
       val minIso = isoRange?.lower ?: 100
       val maxIso = isoRange?.upper ?: 3200
       
       val exposureRange = characteristics.get(CameraCharacteristics.SENSOR_INFO_EXPOSURE_TIME_RANGE)
       val minExposure = exposureRange?.lower ?: 1000000L
       val maxExposure = exposureRange?.upper ?: 1000000000L
       
       val focalLengths = characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_FOCAL_LENGTHS) ?: floatArrayOf()
       val apertures = characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_APERTURES) ?: floatArrayOf()
       val minFocusDistance = characteristics.get(CameraCharacteristics.LENS_INFO_MINIMUM_FOCUS_DISTANCE) ?: 0f
       
       val opticalStabilization = characteristics.get(CameraCharacteristics.LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION)
       val hasOis = opticalStabilization?.contains(CameraCharacteristics.LENS_OPTICAL_STABILIZATION_MODE_ON) == true
       
       return CameraData(
           index = index,
           androidCameraId = cameraId,
           cameraId = cameraId,
           logicalId = logicalId,
           physicalId = physicalId,
           facing = facing,
           width = rawSize.width,
           height = rawSize.height,
           whiteLevel = whiteLevel,
           blackLevel = blackLevel,
           bayerPattern = bayerPattern,
           supportsRaw = supportsRaw,
           minIso = minIso,
           maxIso = maxIso,
           minExposure = minExposure,
           maxExposure = maxExposure,
           sensorWidth = sensorWidth,
           sensorHeight = sensorHeight,
           focalLengths = focalLengths,
           apertures = apertures,
           minFocusDistance = minFocusDistance,
           hasOis = hasOis,
           hardwareLevel = hardwareLevel,
           sensorOrientation = sensorOrientation,
           pixelArrayWidth = pixelArrayWidth,
           maxRes = maxRes,
           isCropped = isCropped
       )
   }

   // True if this camera advertises the ultra-high-resolution (non-binned) sensor mode, i.e. a distinct MAXIMUM_RESOLUTION RAW readout we can offer as a second entry.
   private fun supportsMaxRes(characteristics: CameraCharacteristics): Boolean {
       if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return false
       val maxMap = characteristics.get(
           CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP_MAXIMUM_RESOLUTION
       )
       // The max-res RAW config (RAW10) lives in the high-resolution size list. Gate on an actual returned size: the Pixel doesn't set the ULTRA_HIGH_RESOLUTION cap bit, so the size's presence is the real signal.
       val maxRawSizes = maxResRawSizes(maxMap, characteristics)
       return maxRawSizes.isNotEmpty()
   }

   // RAW sizes for the maximum-resolution mode. The StreamConfigurationMap on the Pixel advertises RAW (format 37) in the max-res map but returns NOTHING from both getHighResolutionOutputSizes() and getOutputSizes() (a known framework quirk). So we also derive the size directly from the max-res pixel-array metadata when the sensor advertises the ULTRA_HIGH_RESOLUTION capability.
   internal fun maxResRawSizes(
       map: android.hardware.camera2.params.StreamConfigurationMap?,
       characteristics: CameraCharacteristics? = null
   ): List<Size> {
       // The max-res config exposes RAW as RAW10 (format 37), not RAW_SENSOR.
       val sizes = mutableListOf<Size>()
       if (map != null) {
           try { map.getHighResolutionOutputSizes(ImageFormat.RAW10)?.let { sizes.addAll(it) } } catch (e: Exception) {}
           map.getOutputSizes(ImageFormat.RAW10)?.let { sizes.addAll(it) }
       }
       // Fallback: the Pixel lists RAW10 in the max-res map but may not return a size from the size methods. Synthesize from the max-res pixel array when the map lists RAW10.
       if (sizes.isEmpty() && characteristics != null
           && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
           val rawListed = map?.outputFormats?.contains(ImageFormat.RAW10) == true
           if (rawListed) {
               val px = characteristics.get(CameraCharacteristics.SENSOR_INFO_PIXEL_ARRAY_SIZE_MAXIMUM_RESOLUTION)
                   ?: characteristics.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE_MAXIMUM_RESOLUTION)?.let { Size(it.width(), it.height()) }
               if (px != null && px.width > 0 && px.height > 0) {
                   sizes.add(Size(px.width, px.height))
               }
           }
       }
       return sizes
   }

   private fun packCameraData(camera: CameraData, output: MutableList<Float>) {
       output.add(Float.NaN)
       output.add(camera.index.toFloat())
       output.add(camera.androidCameraId.length.toFloat())
       for (char in camera.androidCameraId) {
           output.add(char.code.toFloat())
       }
       output.add(camera.facing.toFloat())
       output.add(camera.width.toFloat())
       output.add(camera.height.toFloat())
       output.add(camera.whiteLevel.toFloat())
       output.add(camera.blackLevel.toFloat())
       output.add(camera.bayerPattern.toFloat())
       output.add(if (camera.supportsRaw) Float.POSITIVE_INFINITY else Float.NEGATIVE_INFINITY)
       output.add(camera.minIso.toFloat())
       output.add(camera.maxIso.toFloat())
       output.add(camera.minExposure.toFloat())
       output.add(camera.maxExposure.toFloat())
       output.add(camera.sensorWidth)
       output.add(camera.sensorHeight)
       output.add(camera.focalLengths.size.toFloat())
       for (fl in camera.focalLengths) {
           output.add(fl)
       }
       output.add(camera.apertures.size.toFloat())
       for (aperture in camera.apertures) {
           output.add(aperture)
       }
       output.add(camera.minFocusDistance)
       output.add(if (camera.hasOis) Float.POSITIVE_INFINITY else Float.NEGATIVE_INFINITY)
       output.add(camera.hardwareLevel.toFloat())
       output.add(camera.sensorOrientation.toFloat())
       output.add(camera.pixelArrayWidth.toFloat())
       output.add(camera.modeCount.toFloat())  // >1 on group head => lens has multiple modes
       output.add(camera.groupId.toFloat())    // modes of the same lens share this
       output.add(if (camera.maxRes) 1f else 0f)     // max-res (non-binned) readout
       output.add(if (camera.isCropped) 1f else 0f)  // cropped sub-FOV readout
       // Physical-camera plumbing (appended after the fields the Rust menu parser reads; it ignores everything up to the -0.0f terminator).
       output.add(if (camera.physicalId != null) Float.POSITIVE_INFINITY else Float.NEGATIVE_INFINITY)
       output.add(camera.logicalId.length.toFloat())
       for (char in camera.logicalId) {
           output.add(char.code.toFloat())
       }
       val physical = camera.physicalId ?: ""
       output.add(physical.length.toFloat())
       for (char in physical) {
           output.add(char.code.toFloat())
       }
       output.add(-0.0f)
   }
}

// Data class for camera settings from Rust
data class CameraSettings(
   val iso: Int,           // Camera2 expects Int
   val shutterNs: Long,    // Camera2 expects Long (nanoseconds)
   val focusDistance: Float // Camera2 expects Float
)

// Data class for camera info parsing
data class CameraInfo(
   val widthPixels: Int,
   val heightPixels: Int, 
   val whiteLevel: Int,
   val blackLevel: Int,
   val bayerPattern: Int,
   val facing: Int,
   val sensorOrientation: Int,
   val minIso: Int,
   val maxIso: Int,
   val minExposure: Long,
   val maxExposure: Long,
   val minFocusDistance: Float,
   val hardwareLevel: Int,
   val logicalId: String = "",
   val physicalId: String? = null,
   val maxRes: Boolean = false
)

// Camera processor - handles Camera2 API
class CameraProcessor(private val service: CameraInterface) {
   
   // Native context pointer for Camera processing ONLY
   private var nativeContextPtr: Long = 0L
   
   // Camera2 components
   private var cameraManager: CameraManager? = null
   private var cameraDevice: CameraDevice? = null
   private var captureSession: CameraCaptureSession? = null
   private var imageReader: ImageReader? = null
   private var backgroundThread: HandlerThread? = null
   private var backgroundHandler: Handler? = null
   private var cameraId: String? = null

   // Settings poll: decouples HAL setting application from frame delivery. Frames clock the old update path once per delivered frame, so at long exposures (seconds per frame) a dial change took several frames to reach the capture request. This poll reads the current ISO/shutter/focus from shared memory at a fixed fast rate and pushes any change immediately via updateCameraSettings (which itself no-ops when nothing changed).
   private val SETTINGS_POLL_MS = 33L // ~30Hz; fine-grained enough that dial changes feel instant regardless of frame rate.
   private var settingsPollActive = false
   private val settingsPollRunnable = object : Runnable {
       override fun run() {
           if (!settingsPollActive) return
           val ptr = nativeContextPtr
           if (ptr != 0L && captureSession != null && !aeWarmingUp) {
               try {
                   val s = CameraInterface.nativeGetCurrentSettings(ptr)
                   updateCameraSettings(s.iso, s.shutterNs, s.focusDistance)
               } catch (e: Exception) {
                   Log.w("CameraInterface", "settings poll failed: $e")
               }
           }
           backgroundHandler?.postDelayed(this, SETTINGS_POLL_MS)
       }
   }

   private fun startSettingsPoll() {
       if (settingsPollActive) return
       settingsPollActive = true
       backgroundHandler?.postDelayed(settingsPollRunnable, SETTINGS_POLL_MS)
   }

   private fun stopSettingsPoll() {
       settingsPollActive = false
       backgroundHandler?.removeCallbacks(settingsPollRunnable)
   }
   // Set when the selected sensor is a hidden physical sub-camera; targeted via OutputConfiguration.setPhysicalCameraId() on its parent logical camera.
   private var physicalCameraId: String? = null
   // True when the selected mode is the maximum-resolution (non-binned) readout: the ImageReader uses the max-res size and every capture request sets SENSOR_PIXEL_MODE.
   private var useMaxResolution: Boolean = false

   // One-shot auto-exposure warm-up: when a camera opens we let Camera2 meter the scene for a few frames, then seed the manual ISO/shutter from what it picked and switch to full manual. Avoids hardcoding an exposure that's wrong on other sensors.
   private var aeWarmingUp = true
   private var aeFrameCount = 0
   private val AE_MAX_WARMUP_FRAMES = 20  // ~fallback if the HAL never reports CONVERGED

   // Capture metadata tracking
   private var lastCaptureResult: CaptureResult? = null
   private val captureCallback = object : CameraCaptureSession.CaptureCallback() {
       override fun onCaptureCompleted(
           session: CameraCaptureSession,
           request: CaptureRequest,
           result: TotalCaptureResult
       ) {
           lastCaptureResult = result

           if (aeWarmingUp) {
               aeFrameCount++
               val aeState = result.get(CaptureResult.CONTROL_AE_STATE)
               val converged = aeState == null ||  // some HALs don't report state
                   aeState == CaptureResult.CONTROL_AE_STATE_CONVERGED ||
                   aeState == CaptureResult.CONTROL_AE_STATE_LOCKED ||
                   aeState == CaptureResult.CONTROL_AE_STATE_FLASH_REQUIRED
               if ((converged && aeFrameCount >= 3) || aeFrameCount >= AE_MAX_WARMUP_FRAMES) {
                   val meteredIso = result.get(CaptureResult.SENSOR_SENSITIVITY) ?: currentIso
                   val meteredShutter = result.get(CaptureResult.SENSOR_EXPOSURE_TIME) ?: currentShutterSpeedNs
                   Log.i("CameraInterface", "AE settled (state=$aeState, frame=$aeFrameCount): ISO=$meteredIso shutter=${meteredShutter}ns")
                   finishAeWarmup(meteredIso, meteredShutter)
               }
           }
       }
   }
   
   // Current camera settings (applied to Camera2)
   private var currentIso: Int = 100
   private var currentShutterSpeedNs: Long = 33000000L // 33ms
   private var currentFocusDistance: Float = 0.0f
   
   // Store reply messenger for initialization complete signal
   private var replyMessenger: Messenger? = null
   
   init {
       cameraManager = service.getSystemService(Context.CAMERA_SERVICE) as CameraManager
   }
   
   fun setNativeContext(contextPtr: Long) {
       nativeContextPtr = contextPtr
   }
   
   fun setReplyMessenger(messenger: Messenger?) {
       replyMessenger = messenger
   }
   
   private val imageAvailableListener = ImageReader.OnImageAvailableListener { reader ->
       val image = reader.acquireLatestImage() ?: return@OnImageAvailableListener

       // During AE warm-up the integrator doesn't exist yet (nativeContextPtr==0). We MUST still close these frames or the ImageReader (maxImages=2) fills up and acquireLatestImage throws, crashing the camera thread.
       if (nativeContextPtr == 0L) {
           image.close()
           return@OnImageAvailableListener
       }

       try {
           val buffer = image.planes[0].buffer
           // RAW10 (the max-res format on this device) is MIPI-packed and row-padded; Rust depacks it. RAW_SENSOR is already 16-bit so no depack. rowStride is in bytes.
           val isRaw10 = image.format == ImageFormat.RAW10
           val rowStride = image.planes[0].rowStride

           // Get capture metadata
           val captureResult = lastCaptureResult
           val actualIso = captureResult?.get(CaptureResult.SENSOR_SENSITIVITY) ?: currentIso
           val actualShutterSpeedNs = captureResult?.get(CaptureResult.SENSOR_EXPOSURE_TIME) ?: currentShutterSpeedNs
           val actualFocusDistance = captureResult?.get(CaptureResult.LENS_FOCUS_DISTANCE) ?: currentFocusDistance

           // Pass frame to Camera processing and get camera settings to apply
           val cameraSettings = CameraInterface.nativeCameraOnFrame(
               nativeContextPtr,
               buffer,
               actualIso,
               actualShutterSpeedNs,
               actualFocusDistance,
               isRaw10,
               rowStride
           )
           
           // Check if there's saved image data to process (any format)
           service.nativeGetSavedDngData()?.let { savedData: SaveDng ->
               val mimeType = service.mimeForFilename(savedData.filename)
               Log.i("CameraInterface", "Processing saved image: ${savedData.dngData.size} bytes, filename: ${savedData.filename}, mime: $mimeType")
               if (service.saveImageToMediaStoreImpl(savedData.dngData, savedData.filename, mimeType)) {
                   Log.i("CameraInterface", "Successfully saved: ${savedData.filename}")
                   // Clear save in progress flag so next save can proceed
                   service.nativeClearSaveInProgress(service.nativeCameraContextPtr)
               } else {
                   Log.e("CameraInterface", "Failed to save DNG: ${savedData.filename}")
                   // Also clear on failure so we don't block forever
                   service.nativeClearSaveInProgress(service.nativeCameraContextPtr)
               }
           }
           
           // Apply camera settings and launch UI immediately
           Log.i("CameraInterface", "Camera settings received: ISO=${cameraSettings.iso}, shutter=${cameraSettings.shutterNs}, focus=${cameraSettings.focusDistance}")

           // Apply settings to camera
           updateCameraSettings(cameraSettings.iso, cameraSettings.shutterNs, cameraSettings.focusDistance)

           // If this is the first frame, launch UI
           if (replyMessenger != null) {
               Log.i("CameraInterface", "First frame - launching UI")
               service.onCameraInitializationComplete(replyMessenger)
               replyMessenger = null // Only send once
           } else {
               Log.d("CameraInterface", "Settings applied - UI already launched")
           }
       } finally {
           image.close()
       }
   }

   // Camera state callback
   private val stateCallback = object : CameraDevice.StateCallback() {
       override fun onOpened(camera: CameraDevice) {
           cameraDevice = camera
           createCaptureSession()
       }
       
        override fun onDisconnected(camera: CameraDevice) {
            Log.e("CameraInterface", "Camera disconnected - kill switch activated")
            Process.killProcess(Process.myPid())
        }

        override fun onError(camera: CameraDevice, error: Int) {
            Log.e("CameraInterface", "Camera error: $error - kill switch activated")  
            Process.killProcess(Process.myPid())
        }
   }
   
   @SuppressLint("MissingPermission")
   fun openCamera(logicalId: String, physicalId: String?, maxRes: Boolean = false) {
       useMaxResolution = maxRes
       // Start background thread with high priority
       backgroundThread = HandlerThread("CameraThread").apply {
           start()
           priority = Thread.MAX_PRIORITY
       }
       backgroundHandler = Handler(backgroundThread!!.looper)

       // Set high priority for camera processing
       backgroundHandler?.post {
           Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_DISPLAY)
       }

       // We always OPEN the logical camera; for a physical sub-camera we route the RAW stream to it via setPhysicalCameraId() when configuring the session.
       cameraId = logicalId
       physicalCameraId = physicalId
       aeWarmingUp = true
       aeFrameCount = 0

       // RAW sizes must come from whichever sensor actually produces the stream - the physical camera if targeting one, else the logical camera. For the max-res mode read from the MaximumResolution stream config (the non-binned native readout).
       val sizeChars = cameraManager!!.getCameraCharacteristics(physicalId ?: logicalId)
       val streamConfigMap = if (maxRes && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
           sizeChars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP_MAXIMUM_RESOLUTION)
       } else {
           sizeChars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
       }
       // The max-res config exposes RAW only as RAW10 (10-bit MIPI-packed), not RAW_SENSOR. Binned uses 16-bit RAW_SENSOR. Choose the format + size accordingly.
       val rawFormat = if (maxRes) ImageFormat.RAW10 else ImageFormat.RAW_SENSOR
       val rawSizes: List<Size> = if (maxRes && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
           service.maxResRawSizes(streamConfigMap, sizeChars)
       } else {
           (streamConfigMap!!.getOutputSizes(rawFormat) ?: arrayOf()).toList()
       }
       // Largest available (max-res maps may list several).
       val rawSize = rawSizes.maxByOrNull { it.width.toLong() * it.height.toLong() }!!
       Log.i("CameraInterface", "openCamera rawSize picked=${rawSize.width}x${rawSize.height} format=${if (maxRes) "RAW10" else "RAW_SENSOR"} maxRes=$maxRes")

       // Create ImageReader
       imageReader = ImageReader.newInstance(
           rawSize.width,
           rawSize.height,
           rawFormat,
           2
       ).apply {
           setOnImageAvailableListener(imageAvailableListener, backgroundHandler)
       }

       cameraManager!!.openCamera(logicalId, stateCallback, backgroundHandler)
       Log.i("CameraInterface", "Opening camera logical=$logicalId physical=${physicalId ?: "none"} (${rawSize.width}x${rawSize.height})")
   }
   
   private fun updateCameraSettings(iso: Int, shutterSpeedNs: Long, focusDistance: Float) {
       // Only update camera hardware if values actually changed
       if (iso == currentIso && shutterSpeedNs == currentShutterSpeedNs && focusDistance == currentFocusDistance) {
           return // No change needed
       }
       
       currentIso = iso
       currentShutterSpeedNs = shutterSpeedNs
       currentFocusDistance = focusDistance
       
       // Apply to current Camera2 session. The new repeating request carries the new manual settings; the HAL applies them on the next exposure it starts (verified instant: a request carrying 1s yields a 1s result on the very next started frame). We deliberately do NOT abortCaptures() here - it only drops not-yet-started queued frames (not the one mid-integration), made zero difference to the switch latency in testing, and calling it on every change would stall the stream. The real latency was the once-per-frame update cadence, now fixed by the fixed-rate settings poll that calls this immediately on change.
       captureSession?.let { session ->
           cameraDevice?.let { device ->
               try {
                   val captureBuilder = buildManualRequest(device)
                   session.setRepeatingRequest(captureBuilder.build(), captureCallback, backgroundHandler)
               } catch (e: Exception) {
                   Log.e("CameraInterface", "Failed to update camera settings: $e")
               }
           }
       }
   }

   // Build a manual capture request targeting the RAW ImageReader, applying the current manual settings. When a physical sub-camera is selected, the same settings are also set as physical-camera keys so the chosen sensor obeys them.
   private fun buildManualRequest(device: CameraDevice): CaptureRequest.Builder {
       val b = device.createCaptureRequest(CameraDevice.TEMPLATE_MANUAL)
       b.addTarget(imageReader!!.surface)
       // Manual sensor keys on the logical request propagate to the single physical RAW stream on this HAL, so per-physical-camera keys aren't needed (and setPhysicalCameraKey is rejected for a non-streaming-config physical id).
       b.set(CaptureRequest.CONTROL_MODE, CameraMetadata.CONTROL_MODE_OFF)
       b.set(CaptureRequest.SENSOR_SENSITIVITY, currentIso)
       b.set(CaptureRequest.SENSOR_EXPOSURE_TIME, currentShutterSpeedNs)
       b.set(CaptureRequest.LENS_FOCUS_DISTANCE, currentFocusDistance)
       applyPixelMode(b)
       return b
   }

   // Select the maximum-resolution (non-binned) sensor pixel mode on a request when the chosen camera entry is the max-res variant. Must be set on EVERY request feeding the max-res-sized ImageReader, or the HAL rejects the size mismatch.
   private fun applyPixelMode(b: CaptureRequest.Builder) {
       if (useMaxResolution && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
           b.set(
               CaptureRequest.SENSOR_PIXEL_MODE,
               CameraMetadata.SENSOR_PIXEL_MODE_MAXIMUM_RESOLUTION
           )
       }
   }
   
   private fun createCaptureSession() {
       val device = cameraDevice ?: return
       val reader = imageReader ?: return
       
       try {
           val outputConfig = OutputConfiguration(reader.surface).apply {
               // Route the RAW stream to the specific physical sensor when one is selected.
               physicalCameraId?.let { setPhysicalCameraId(it) }
           }
           val sessionConfig = SessionConfiguration(
               SessionConfiguration.SESSION_REGULAR,
               listOf(outputConfig),
               ContextCompat.getMainExecutor(service),
               object : CameraCaptureSession.StateCallback() {
                   override fun onConfigured(session: CameraCaptureSession) {
                       captureSession = session
                       startCapture()
                       // Poll settings independently of frame delivery so dial changes reach the HAL immediately even at long exposures (the poll no-ops during AE warm-up and when nothing changed).
                       startSettingsPoll()
                       Log.i("CameraInterface", "Capture session configured")
                   }
                   
                   override fun onConfigureFailed(session: CameraCaptureSession) {
                       Log.e("CameraInterface", "Capture session configuration failed")
                   }
               }
           )
           device.createCaptureSession(sessionConfig)
       } catch (e: Exception) {
           Log.e("CameraInterface", "Failed to create capture session: $e")
       }
   }
   
   private fun startCapture() {
       val session = captureSession ?: return
       val device = cameraDevice ?: return

       try {
           // Begin with an auto-exposure warm-up so we can meter the scene and pick sensible opening ISO/shutter; captureCallback flips us to manual once AE settles. Focus stays manual (infinity) throughout - we only auto exposure.
           val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
           builder.addTarget(imageReader!!.surface)
           builder.set(CaptureRequest.CONTROL_MODE, CameraMetadata.CONTROL_MODE_AUTO)
           builder.set(CaptureRequest.CONTROL_AE_MODE, CameraMetadata.CONTROL_AE_MODE_ON)
           builder.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_OFF)
           builder.set(CaptureRequest.LENS_FOCUS_DISTANCE, currentFocusDistance)
           applyPixelMode(builder)
           session.setRepeatingRequest(builder.build(), captureCallback, backgroundHandler)
           Log.i("CameraInterface", "Started AE warm-up")
       } catch (e: Exception) {
           Log.e("CameraInterface", "Failed to start capture: $e")
       }
   }

   // AE has settled: seed the integrator with the metered values, then switch the repeating request to full manual. Runs on the camera background thread.
   private fun finishAeWarmup(meteredIso: Int, meteredShutterNs: Long) {
       aeWarmingUp = false
       currentIso = meteredIso
       currentShutterSpeedNs = meteredShutterNs

       // Create the native integrator seeded with the metered values (this also calls setNativeContext on us), then start pushing frames via the manual request.
       service.initIntegrator(meteredIso, meteredShutterNs)

       val session = captureSession ?: return
       val device = cameraDevice ?: return
       try {
           val captureBuilder = buildManualRequest(device)
           session.setRepeatingRequest(captureBuilder.build(), captureCallback, backgroundHandler)
           Log.i("CameraInterface", "Switched to manual capture (ISO=$meteredIso shutter=${meteredShutterNs}ns)")
       } catch (e: Exception) {
           Log.e("CameraInterface", "Failed to start manual capture: $e")
       }
   }
}