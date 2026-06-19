package com.lumis.camera

data class SaveDng(
    val dngData: ByteArray,
    val filename: String
) {
    override fun equals(other: Any?): Boolean {
        if (this === other) return true
        if (javaClass != other?.javaClass) return false

        other as SaveDng

        if (!dngData.contentEquals(other.dngData)) return false
        if (filename != other.filename) return false

        return true
    }

    override fun hashCode(): Int {
        var result = dngData.contentHashCode()
        result = 31 * result + filename.hashCode()
        return result
    }
}