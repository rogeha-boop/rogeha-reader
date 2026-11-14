/* MSVC's C headers don't define max_align_t (only its C++ headers do), but the mupdf
 * crate expects it in the generated bindings. On MSVC it is `double` (8-byte alignment).
 * This header is force-included into bindgen via BINDGEN_EXTRA_CLANG_ARGS (.cargo/config.toml). */
#if defined(_MSC_VER) && !defined(__cplusplus)
typedef double max_align_t;
#endif
