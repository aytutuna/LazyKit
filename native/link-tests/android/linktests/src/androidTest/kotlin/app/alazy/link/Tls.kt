package app.alazy.link

// The entry the relay client's library exports for Android's certificate check (rust/lazy-link-ffi/src/android.rs).
object Tls {
    init {
        System.loadLibrary("lazy_link_ffi")
    }

    @JvmStatic
    external fun init(context: android.content.Context)
}
