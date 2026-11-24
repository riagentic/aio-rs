package aio.rt;

import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.CancellationSignal;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyPermanentlyInvalidatedException;
import android.security.keystore.KeyProperties;
import java.security.KeyStore;
import java.util.concurrent.CancellationException;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/** The app's Rust library: its view runs there, frames cross as base64 text. */
public final class Host {
    static {
        System.loadLibrary("app");
    }

    private Host() {}

    /** Boot the app (once per process); returns the page to load. */
    public static native String start(String filesDir);

    /** A page is loaded and its port posted: it gets a fresh view. */
    public static native void connect();

    /** A frame from the page. */
    public static native void fromPage(String frame);

    /** Back was pressed: the app's handlers are asked; if none takes it, {@link #notTaken} runs. */
    public static native void back();

    /** The app was opened with this link (one of its schemes). Data, never code. */
    public static native void link(String url);

    /** May the page hold this permission (the app's window says: "media" is the camera)? */
    public static native boolean allows(String permission);

    /** The system picker answered question {@code id}: each document's name and
     *  detached descriptor (null: cancelled), or {@code error}. */
    public static native void answer(long id, String[] names, int[] fds, String error);

    /** Back's default (the page's history, then leave), set by the activity. */
    static volatile Runnable backDefault;

    /** No handler took Back (called by the library, from any thread). */
    static void notTaken() {
        Runnable r = backDefault;
        if (r != null) r.run();
    }

    /** The activity is finishing: let the app's stores finish. */
    public static native void stop();

    interface Sink {
        void frame(String frame);
    }

    static volatile Sink sink;

    /** A frame for the page, from any thread (called by the library). */
    static void toPage(String frame) {
        Sink s = sink;
        if (s != null) s.frame(frame);
    }

    /** The activity showing now (the system picker opens over it). */
    static volatile MainActivity activity;

    /** Ask the system picker (called by the library, from any thread): what 0
     *  one file, 1 several, 2 where to save {@code name}. False: no window. */
    static boolean ask(long id, int what, String title, String[] extensions, String name) {
        MainActivity a = activity;
        if (a == null) return false;
        a.runOnUiThread(() -> a.ask(id, what, title, extensions, name));
        return true;
    }

    /** The Keystore's key of aio::secrets, made once and never handed out;
     *  {@code biometric}: the one used only after the user's fingerprint or face. */
    private static SecretKey key(boolean biometric) throws Exception {
        String alias = biometric ? "aio.secrets.biometric" : "aio.secrets";
        KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
        ks.load(null);
        if (ks.getKey(alias, null) instanceof SecretKey k) return k;
        KeyGenParameterSpec.Builder spec = new KeyGenParameterSpec.Builder(
                        alias, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256);
        // Each use asked for (a strong biometric); enrolling another invalidates it.
        if (biometric) spec.setUserAuthenticationRequired(true);
        KeyGenerator g = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        g.init(spec.build());
        return g.generateKey();
    }

    /** {@code plain} sealed (AES-GCM): its 12-byte IV, then the text and tag
     *  (called by the library on a worker thread; blocks while the user is asked). */
    static byte[] seal(boolean biometric, byte[] plain) throws Exception {
        if (biometric) canAsk();
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        try {
            c.init(Cipher.ENCRYPT_MODE, key(biometric));
        } catch (KeyPermanentlyInvalidatedException e) {
            if (!biometric) throw e;
            // A fingerprint enrolled since: what it guarded is lost; a new key guards this.
            KeyStore ks = KeyStore.getInstance("AndroidKeyStore");
            ks.load(null);
            ks.deleteEntry("aio.secrets.biometric");
            c.init(Cipher.ENCRYPT_MODE, key(true));
        }
        byte[] text = biometric ? asked(c, plain) : c.doFinal(plain);
        byte[] iv = c.getIV();
        byte[] out = new byte[iv.length + text.length];
        System.arraycopy(iv, 0, out, 0, iv.length);
        System.arraycopy(text, 0, out, iv.length, text.length);
        return out;
    }

    /** What {@link #seal} made, opened (as it is called). */
    static byte[] open(boolean biometric, byte[] sealed) throws Exception {
        if (sealed.length < 12 + 16) throw new javax.crypto.AEADBadTagException("cut short");
        if (biometric) canAsk();
        Cipher c = Cipher.getInstance("AES/GCM/NoPadding");
        c.init(Cipher.DECRYPT_MODE, key(biometric), new GCMParameterSpec(128, sealed, 0, 12));
        byte[] text = java.util.Arrays.copyOfRange(sealed, 12, sealed.length);
        return biometric ? asked(c, text) : c.doFinal(text);
    }

    /** A fingerprint or face can be asked for here: else why not. */
    @SuppressWarnings("deprecation")
    private static void canAsk() {
        MainActivity a = activity;
        if (a == null) throw new IllegalStateException("no window to ask the user in");
        if (a.checkSelfPermission(android.Manifest.permission.USE_BIOMETRIC)
                != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            throw new UnsupportedOperationException("this app doesn't ask for a fingerprint or face:"
                    + " [build.android] biometric = true in its aio.toml");
        }
        BiometricManager m = a.getSystemService(BiometricManager.class);
        int can = android.os.Build.VERSION.SDK_INT >= 30
                ? m.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                : m.canAuthenticate();
        if (can != BiometricManager.BIOMETRIC_SUCCESS) {
            throw new UnsupportedOperationException(can == BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED
                    ? "no fingerprint or face is enrolled on this phone"
                    : "this phone has no strong fingerprint or face reader (" + can + ")");
        }
    }

    /** {@code c} run on {@code in} once the user's fingerprint or face unlocks
     *  its key (the system's prompt, over the activity). */
    private static byte[] asked(Cipher c, byte[] in) throws Exception {
        MainActivity a = activity;
        if (a == null) throw new IllegalStateException("no window to ask the user in");
        CompletableFuture<byte[]> done = new CompletableFuture<>();
        a.runOnUiThread(() -> {
            try {
                CharSequence app = a.getApplicationInfo().loadLabel(a.getPackageManager());
                BiometricPrompt p = new BiometricPrompt.Builder(a)
                        .setTitle(app)
                        .setNegativeButton(a.getString(android.R.string.cancel), a.getMainExecutor(),
                                (d, w) -> done.completeExceptionally(new CancellationException("the user cancelled")))
                        .build();
                p.authenticate(new BiometricPrompt.CryptoObject(c), new CancellationSignal(), a.getMainExecutor(),
                        new BiometricPrompt.AuthenticationCallback() {
                            @Override
                            public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult r) {
                                try {
                                    done.complete(c.doFinal(in));
                                } catch (Exception e) {
                                    done.completeExceptionally(e);
                                }
                            }

                            @Override
                            public void onAuthenticationError(int code, CharSequence why) {
                                boolean cancelled = code == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED
                                        || code == BiometricPrompt.BIOMETRIC_ERROR_CANCELED
                                        || code == BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT
                                        || code == BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT_PERMANENT;
                                done.completeExceptionally(cancelled
                                        ? new CancellationException(why.toString())
                                        : new SecurityException(why.toString()));
                            }
                        });
            } catch (Exception e) {
                done.completeExceptionally(e);
            }
        });
        try {
            return done.get();
        } catch (ExecutionException e) {
            throw e.getCause() instanceof Exception x ? x : e;
        }
    }
}
