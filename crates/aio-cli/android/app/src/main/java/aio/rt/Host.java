package aio.rt;

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
}
