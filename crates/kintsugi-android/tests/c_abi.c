/*
 * The C ABI, exercised from C.
 *
 * The Kotlin app talks to the JNI layer, and the JNI layer calls exactly the
 * same functions this program calls, so this file is the honest way to test
 * the boundary on a desktop: if it links, runs, and prints, the exported
 * symbols, the ownership rule, and the error paths all work.
 *
 * Build (macOS):
 *   cargo build -p kintsugi-android
 *   clang -Wall -Wextra -Werror tests/c_abi.c -o /tmp/c_abi \
 *       -Ltarget/debug -lkintsugi_android -Wl,-rpath,target/debug
 *   /tmp/c_abi
 *
 * On Linux the library is `libkintsugi_android.so` and the same command works
 * with `-Ltarget/debug -lkintsugi_android`.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The ABI this library exports. Kept in the test, not in a header, so a
 * change in the Rust signatures shows up here as a compile error. */
extern char *kintsugi_version(void);
extern char *kintsugi_detect(const char *dir);
extern char *kintsugi_play(const char *dir, const char *script);
extern char *kintsugi_demo(const char *dir);
extern char *kintsugi_upscale(const char *dir, const char *asset,
                              unsigned int factor, const char *method,
                              const char *out_path);
extern void kintsugi_free(char *text);

static int failures = 0;

static void expect(int condition, const char *what) {
    if (!condition) {
        fprintf(stderr, "FAIL: %s\n", what);
        failures++;
    } else {
        printf("ok: %s\n", what);
    }
}

/* Every returned string is owned by us and released here. */
static char *take(char *owned) {
    if (owned == NULL) {
        fprintf(stderr, "FAIL: the library returned NULL\n");
        failures++;
    }
    return owned;
}

int main(void) {
    char *version = take(kintsugi_version());
    if (version) {
        printf("version: %s\n", version);
        expect(strncmp(version, "kintsugi ", 9) == 0, "version is prefixed");
        kintsugi_free(version);
    }

    const char *dir = "/tmp/kintsugi-c-abi-demo";

    char *demo = take(kintsugi_demo(dir));
    if (demo) {
        expect(strstr(demo, "bluegale") != NULL, "demo detects the seam");
        expect(strstr(demo, "\xe9\x87\x91\xe7\xb6\x99\xe3\x81\x8e") != NULL,
               "demo plays the Japanese script (UTF-8 across the boundary)");
        expect(strstr(demo, "glazed title.bbm") != NULL, "demo glazes an image");
        kintsugi_free(demo);
    }

    char *detect = take(kintsugi_detect(dir));
    if (detect) {
        expect(strstr(detect, "certain") != NULL, "detection is certain");
        kintsugi_free(detect);
    }

    char *play = take(kintsugi_play(dir, "story.bdt"));
    if (play) {
        expect(strstr(play, "engine: bluegale") != NULL, "play names the engine");
        kintsugi_free(play);
    }

    char *png = malloc(strlen(dir) + 32);
    strcpy(png, dir);
    strcat(png, "/title-c.png");
    char *upscaled =
        take(kintsugi_upscale(dir, "title.bbm", 3, "anime4k", png));
    if (upscaled) {
        /* "4×3 → 12×9": the escapes are split so `\x97` cannot swallow the
         * following digit as another hex digit. */
        expect(strstr(upscaled, "4\xc3\x97" "3 \xe2\x86\x92 12\xc3\x97" "9") != NULL,
               "upscale reports 4x3 -> 12x9");
        FILE *file = fopen(png, "rb");
        expect(file != NULL, "the PNG was written");
        if (file) {
            unsigned char signature[8] = {0};
            size_t read = fread(signature, 1, 8, file);
            fclose(file);
            const unsigned char expected[8] = {0x89, 'P', 'N', 'G',
                                              0x0D, 0x0A, 0x1A, 0x0A};
            expect(read == 8 && memcmp(signature, expected, 8) == 0,
                   "the PNG starts with the PNG signature");
        }
        kintsugi_free(upscaled);
    }
    free(png);

    /* Failures must arrive as text, never as a crash or a NULL. */
    char *missing = take(kintsugi_play("/definitely/not/a/game", NULL));
    if (missing) {
        expect(strncmp(missing, "error: ", 7) == 0, "a bad path returns an error string");
        kintsugi_free(missing);
    }

    /* NULL arguments must be refused, not dereferenced. */
    char *null_dir = take(kintsugi_detect(NULL));
    if (null_dir) {
        expect(strncmp(null_dir, "error: ", 7) == 0, "NULL is refused politely");
        kintsugi_free(null_dir);
    }

    /* Freeing NULL is a no-op, as the contract says. */
    kintsugi_free(NULL);
    printf("ok: free(NULL) is a no-op\n");

    if (failures > 0) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    printf("\nall C ABI checks passed\n");
    return 0;
}
