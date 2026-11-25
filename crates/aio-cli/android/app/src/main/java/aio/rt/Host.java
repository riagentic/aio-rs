package aio.rt;

import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
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
import org.json.JSONArray;
import org.json.JSONObject;

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

    /** A USB device came or went: aio::hw's watches look again. */
    public static native void usbChanged();

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

    /** The app's context for aio::hw, kept from its first call on: devices
     *  plugged in and out are heard from then (usbChanged). */
    private static Context usb;

    private static synchronized Context usb() {
        if (usb != null) return usb;
        MainActivity a = activity;
        if (a == null) throw new IllegalStateException("no activity yet to reach USB through");
        Context c = a.getApplicationContext();
        IntentFilter f = new IntentFilter(UsbManager.ACTION_USB_DEVICE_ATTACHED);
        f.addAction(UsbManager.ACTION_USB_DEVICE_DETACHED);
        // The system's own broadcasts (protected: no app can send them).
        c.registerReceiver(new BroadcastReceiver() {
            @Override
            public void onReceive(Context ctx, Intent i) {
                usbChanged();
            }
        }, f, Context.RECEIVER_EXPORTED);
        usb = c;
        return c;
    }

    /** The USB devices plugged in (called by the library, from any thread), as
     *  JSON: id, vendor, product, class, strings (the serial only once the
     *  app may use the device), each interface's number, class, subclass, protocol. */
    static String usbList() throws Exception {
        UsbManager m = usb().getSystemService(UsbManager.class);
        JSONArray all = new JSONArray();
        for (UsbDevice d : m.getDeviceList().values()) {
            String serial;
            try {
                serial = d.getSerialNumber();
            } catch (SecurityException e) {
                serial = null;
            }
            JSONArray faces = new JSONArray();
            for (int i = 0; i < d.getInterfaceCount(); i++) {
                UsbInterface f = d.getInterface(i);
                if (f.getAlternateSetting() != 0) continue;
                faces.put(new JSONArray().put(f.getId()).put(f.getInterfaceClass())
                        .put(f.getInterfaceSubclass()).put(f.getInterfaceProtocol()));
            }
            all.put(new JSONObject().put("id", d.getDeviceId()).put("vendor", d.getVendorId())
                    .put("product", d.getProductId()).put("class", d.getDeviceClass())
                    .put("manufacturer", orNull(d.getManufacturerName())).put("name", orNull(d.getProductName()))
                    .put("serial", orNull(serial)).put("interfaces", faces));
        }
        return all.toString();
    }

    private static Object orNull(String s) {
        return s == null ? JSONObject.NULL : s;
    }

    /** USB device {@code id} opened (called by the library on a worker thread:
     *  it blocks while the system asks the user, the first time): its
     *  usbfs descriptor, detached for the library. */
    static int usbOpen(int id) throws Exception {
        Context c = usb();
        UsbManager m = c.getSystemService(UsbManager.class);
        UsbDevice d = null;
        for (UsbDevice x : m.getDeviceList().values()) if (x.getDeviceId() == id) d = x;
        if (d == null) throw new java.io.FileNotFoundException("the USB device was unplugged");
        if (!m.hasPermission(d)) askUsb(c, m, d);
        UsbDeviceConnection conn = m.openDevice(d);
        if (conn == null) throw new java.io.IOException("Android did not open the USB device " + d.getDeviceName());
        try {
            return ParcelFileDescriptor.fromFd(conn.getFileDescriptor()).detachFd();
        } finally {
            conn.close();
        }
    }

    /** The system asks the user whether the app may use {@code d}: their answer, waited for. */
    private static void askUsb(Context c, UsbManager m, UsbDevice d) throws Exception {
        // One action per device: two questions at once get their own answers.
        String action = c.getPackageName() + ".AIO_USB." + d.getDeviceId();
        CompletableFuture<Boolean> answered = new CompletableFuture<>();
        BroadcastReceiver r = new BroadcastReceiver() {
            @Override
            public void onReceive(Context ctx, Intent i) {
                answered.complete(i.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false));
            }
        };
        c.registerReceiver(r, new IntentFilter(action), Context.RECEIVER_NOT_EXPORTED);
        try {
            // Mutable: the system adds the answer; explicit: only this app gets it.
            Intent i = new Intent(action).setPackage(c.getPackageName());
            m.requestPermission(d, PendingIntent.getBroadcast(c, d.getDeviceId(), i,
                    PendingIntent.FLAG_MUTABLE | PendingIntent.FLAG_UPDATE_CURRENT));
            if (!answered.get()) {
                String name = d.getProductName() != null ? d.getProductName() : d.getDeviceName();
                throw new SecurityException("the user did not let the app use the USB device " + name);
            }
        } finally {
            c.unregisterReceiver(r);
        }
    }
}
