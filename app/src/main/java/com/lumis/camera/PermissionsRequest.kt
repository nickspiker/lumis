package com.lumis.camera

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.Manifest
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import android.os.Build

class PermissionsRequest : Activity() {
    
    private val PERMISSIONS_REQUEST_CODE = 1000
    
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        
        // Just a black screen - no UI needed
        setContentView(android.view.View(this).apply {
            setBackgroundColor(android.graphics.Color.BLACK)
        })
        
        checkAndRequestPermissions()
    }
    
    private fun checkAndRequestPermissions() {
        val requiredPermissions = mutableListOf(
            Manifest.permission.CAMERA,
            // Location for geotagging saved photos. Optional - the app launches regardless of the result
            // (onRequestPermissionsResult always proceeds); captures just go untagged if denied.
            Manifest.permission.ACCESS_FINE_LOCATION
        )
        
        // Add storage permissions for older Android versions
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            requiredPermissions.add(Manifest.permission.WRITE_EXTERNAL_STORAGE)
            requiredPermissions.add(Manifest.permission.READ_EXTERNAL_STORAGE)
        }
        
        val missingPermissions = requiredPermissions.filter {
            ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
        }
        
        if (missingPermissions.isEmpty()) {
            // All permissions granted - launch UserInterface
            launchUserInterface()
        } else {
            // Request missing permissions
            ActivityCompat.requestPermissions(
                this,
                missingPermissions.toTypedArray(),
                PERMISSIONS_REQUEST_CODE
            )
        }
    }
    
    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        
        when (requestCode) {
            PERMISSIONS_REQUEST_CODE -> {
                // Launch UserInterface regardless of permission results
                launchUserInterface()
            }
        }
    }
    
    
    private fun launchUserInterface() {
        val intent = Intent(this, UserInterface::class.java)
        // Clear the entire task stack when launching UserInterface
        intent.flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK
        startActivity(intent)
        finish()
    }
}