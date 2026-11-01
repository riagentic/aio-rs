package aio.rt;

import android.Manifest;
import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.provider.OpenableColumns;
import android.util.Log;
import android.view.View;
import android.view.ViewTreeObserver;
import android.view.Window;
import android.webkit.ConsoleMessage;
import android.webkit.MimeTypeMap;
import android.webkit.PermissionRequest;
import android.webkit.RenderProcessGoneDetail;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebMessage;
import android.webkit.WebMessagePort;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.TextView;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/** The window: the system WebView showing the app's page, wired to its library. */
public final class MainActivity extends Activity {
    /** The page's origin: reserved (.invalid), so it is never a real site. */
    private static final String ORIGIN = "https://app.invalid";
    /** What the page says once its first frame is drawn (never a frame: base64 is 4n long). */
    private static final String PAINTED = "aio-painted";
    /** The system's splash stays until the first frame, at most this long. */
    private static final long SPLASH_CAP_MS = 10_000;
    /** A second renderer loss within this long is a page that kills it on load. */
    private static final long RENDERER_LOSS_LOOP_MS = 10_000;
    /** Results of the system picker: the app's question, the page's file input; the camera's prompt. */
    private static final int ASK = 1, FILES = 2, CAMERA = 3;

    /** When the renderer was last lost: outlives the activity recreate() replaces. */
    private static long lastRendererLoss;
    /** The app's question open in the system picker: its id and kind (-1: none). */
    private static long asking = -1;
    private static int askingWhat;

    private final Handler main = new Handler(Looper.getMainLooper());
    private WebView web;
    private WebMessagePort port;
    /** A real load started (not a same-page `#` change, which finishes too). */
    private boolean loading;
    /** The first frame is drawn (or the cap passed): the splash may go. */
    private boolean painted;
    /** The app's splash ([build.splash]) over the page, until its first frame
     *  and at least its minimum; null: none (the system's holds instead). */
    private View splash;
    private long splashShown, splashMin;
    /** The page's file input, while the system picker is open: answered exactly once. */
    private ValueCallback<Uri[]> pendingFiles;
    private PermissionRequest pendingCamera;
    /** The font scale the WebView's text zoom was last set from. */
    private float zoomedFontScale;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        String page;
        try {
            page = Host.start(getFilesDir().getAbsolutePath());
        } catch (RuntimeException e) {
            Log.e("aio", "the app did not start", e);
            TextView t = new TextView(this);
            t.setText(e.getMessage());
            t.setPadding(48, 48, 48, 48);
            setContentView(t);
            return;
        }
        Host.activity = this;
        // A cold start by a link (a recreate carries a state: it was given).
        if (state == null) link(getIntent());
        web = new WebView(this);
        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setAllowFileAccess(false);
        s.setAllowContentAccess(false);
        s.setGeolocationEnabled(false);
        zoomedFontScale = getResources().getConfiguration().fontScale;
        web.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest r) {
                // A link leaves the app: the app that opens it does.
                try {
                    startActivity(new Intent(Intent.ACTION_VIEW, r.getUrl()));
                } catch (ActivityNotFoundException e) {
                    Log.w("aio", "no app on this device opens " + r.getUrl());
                }
                return true;
            }

            @Override
            public void onPageStarted(WebView v, String url, android.graphics.Bitmap icon) {
                loading = true;
            }

            @Override
            public void onPageFinished(WebView v, String url) {
                if (!loading) return;
                loading = false;
                wire();
            }

            // The page runs in the WebView's renderer process, which Android
            // takes back when memory is short; unanswered, the whole app is
            // aborted. The dead WebView goes and the activity is built again:
            // the page reloads, and what the app committed is in its store.
            @Override
            public boolean onRenderProcessGone(WebView v, RenderProcessGoneDetail d) {
                long now = SystemClock.elapsedRealtime();
                boolean loop = lastRendererLoss != 0 && now - lastRendererLoss < RENDERER_LOSS_LOOP_MS;
                lastRendererLoss = now;
                Log.e("aio", "the page's renderer " + (d.didCrash() ? "crashed" : "was killed by the system (memory)")
                        + (loop ? " again within " + RENDERER_LOSS_LOOP_MS / 1000 + " s: not reloading in a loop; the app stops"
                                : ": the page is reloaded"));
                if (loop) return false;
                ((android.view.ViewGroup) v.getParent()).removeView(v);
                v.destroy();
                web = null;
                recreate();
                return true;
            }
        });
        web.setWebChromeClient(new WebChromeClient() {
            // The page's console, in the app's log (as on the desktop).
            @Override
            public boolean onConsoleMessage(ConsoleMessage m) {
                String text = m.message();
                if (text.length() > 8192) text = text.substring(0, 8192) + "…";
                Log.i("aio", "[page] " + m.messageLevel() + ": " + text);
                return true;
            }

            // `<input type="file">`: the system's picker (no permission: the
            // user hands over exactly what they pick).
            @Override
            public boolean onShowFileChooser(WebView v, ValueCallback<Uri[]> callback, FileChooserParams p) {
                if (pendingFiles != null) pendingFiles.onReceiveValue(null);
                pendingFiles = callback;
                Intent i = p.createIntent();
                // An `accept` of extensions is no MIME type: any file then.
                if (i.getType() == null || !i.getType().contains("/")) i.setType("*/*");
                if (p.getMode() == FileChooserParams.MODE_OPEN_MULTIPLE) i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                try {
                    startActivityForResult(i, FILES);
                } catch (ActivityNotFoundException e) {
                    Log.e("aio", "<input type=file>: no app on this device picks a file (" + e + ")");
                    pendingFiles = null;
                    callback.onReceiveValue(null);
                }
                return true;
            }

            // The camera (getUserMedia): only when the app's window allows
            // "media" (as on the desktop) and the APK declares the camera
            // ([build.android] camera); Android asks the user once.
            @Override
            public void onPermissionRequest(PermissionRequest r) {
                List<String> asked = Arrays.asList(r.getResources());
                // The WebView grants a request whole or not at all: anything
                // besides the camera denies it, before a prompt it can't use.
                if (!asked.equals(java.util.Collections.singletonList(PermissionRequest.RESOURCE_VIDEO_CAPTURE))) {
                    Log.w("aio", "permission DENIED to the page: " + asked + ": this app grants the camera"
                            + " (video capture) only, and the WebView refuses a request it cannot grant whole:"
                            + " ask for { video: true } alone");
                    r.deny();
                    return;
                }
                String why = !Host.allows("media") ? "the app's window does not allow \"media\" (Window { permissions })"
                        : !declared(Manifest.permission.CAMERA) ? "the APK does not declare it ([build.android] camera = true)"
                        : null;
                if (why != null) {
                    Log.e("aio", "camera DENIED to the page: " + why);
                    r.deny();
                } else if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
                    r.grant(r.getResources());
                } else {
                    if (pendingCamera != null) pendingCamera.deny();
                    pendingCamera = r;
                    requestPermissions(new String[] {Manifest.permission.CAMERA}, CAMERA);
                }
            }
        });
        // A download the page starts is never saved behind the app's back:
        // the app saves files (aio_shell::page::save). Said, never silent.
        web.setDownloadListener((url, agent, disposition, mime, length) ->
                Log.e("aio", "download NOT saved: " + url + ": the app saves files (aio_shell::page::save)"));
        // Edge to edge (Android 15): the page stays clear of the system bars
        // and the keyboard.
        android.widget.FrameLayout frame = new android.widget.FrameLayout(this);
        frame.addView(web);
        frame.setOnApplyWindowInsetsListener((v, insets) -> {
            android.graphics.Insets bars = insets.getInsets(android.view.WindowInsets.Type.systemBars()
                    | android.view.WindowInsets.Type.displayCutout()
                    | android.view.WindowInsets.Type.ime());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return android.view.WindowInsets.CONSUMED;
        });
        setContentView(frame);
        splash = splash(frame);
        // The system's splash (the app's icon on its background) stays until
        // the page's first frame is drawn, never a blank page between; the
        // app's own splash, drawn at once, holds instead.
        final View content = findViewById(android.R.id.content);
        final long shown = SystemClock.uptimeMillis();
        content.getViewTreeObserver().addOnPreDrawListener(new ViewTreeObserver.OnPreDrawListener() {
            @Override
            public boolean onPreDraw() {
                if (!painted && splash == null) return false;
                content.getViewTreeObserver().removeOnPreDrawListener(this);
                if (splash == null) Log.i("aio", "splash: gone " + (SystemClock.uptimeMillis() - shown) + " ms after launch");
                return true;
            }
        });
        main.postDelayed(() -> {
            if (!painted) Log.w("aio", "splash: no first frame in " + SPLASH_CAP_MS / 1000 + " s: closed anyway");
            painted();
        }, SPLASH_CAP_MS);
        web.loadDataWithBaseURL(ORIGIN, page, "text/html", "utf-8", null);
    }

    private void painted() {
        if (painted || web == null) return;
        painted = true;
        findViewById(android.R.id.content).invalidate();
        if (splash != null) {
            long left = splashMin - (SystemClock.uptimeMillis() - splashShown);
            main.postDelayed(() -> dropSplash("the page's first frame"), Math.max(0, left));
        }
    }

    /** The app's splash over {@code frame}, from its assets (aio-splash.png and
     *  .json): its image at its size (halved for @2x, at most 80% of the
     *  screen's width) and its text, on its background, light or dark as the
     *  page is. Null: the app has none. */
    private View splash(android.widget.FrameLayout frame) {
        org.json.JSONObject s;
        android.graphics.Bitmap png;
        try (java.io.InputStream j = getAssets().open("aio-splash.json");
                java.io.InputStream p = getAssets().open("aio-splash.png")) {
            java.io.ByteArrayOutputStream b = new java.io.ByteArrayOutputStream();
            byte[] buf = new byte[4096];
            for (int n; (n = j.read(buf)) > 0; ) b.write(buf, 0, n);
            s = new org.json.JSONObject(b.toString("UTF-8"));
            png = android.graphics.BitmapFactory.decodeStream(p);
        } catch (java.io.IOException | org.json.JSONException e) {
            return null;
        }
        if (png == null) return null;
        int mode = getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_NIGHT_MASK;
        boolean night = s.isNull("night") ? mode == android.content.res.Configuration.UI_MODE_NIGHT_YES : s.optBoolean("night");
        org.json.JSONArray c = s.optJSONArray(night ? "dark" : "light");
        android.widget.LinearLayout l = new android.widget.LinearLayout(this);
        l.setOrientation(android.widget.LinearLayout.VERTICAL);
        l.setGravity(android.view.Gravity.CENTER);
        l.setBackgroundColor(android.graphics.Color.parseColor(c.optString(0)));
        android.widget.ImageView image = new android.widget.ImageView(this);
        image.setImageBitmap(png);
        android.util.DisplayMetrics dm = getResources().getDisplayMetrics();
        float px = dm.density / Math.max(1, s.optInt("scale", 1));
        px = Math.min(px, 0.8f * dm.widthPixels / png.getWidth());
        l.addView(image, new android.widget.LinearLayout.LayoutParams(
                Math.round(png.getWidth() * px), Math.round(png.getHeight() * px)));
        if (!s.isNull("text")) {
            TextView t = new TextView(this);
            t.setText(s.optString("text"));
            t.setTextColor(android.graphics.Color.parseColor(c.optString(1)));
            t.setTextSize(14);
            t.setGravity(android.view.Gravity.CENTER);
            t.setPadding(0, Math.round(14 * dm.density), 0, 0);
            l.addView(t);
        }
        frame.addView(l, new android.widget.FrameLayout.LayoutParams(-1, -1));
        splashMin = s.optLong("min");
        splashShown = SystemClock.uptimeMillis();
        main.postDelayed(() -> dropSplash("its cap"), s.optLong("max", SPLASH_CAP_MS));
        Log.i("aio", "splash: up");
        return l;
    }

    private void dropSplash(String why) {
        if (splash == null) return;
        ((android.view.ViewGroup) splash.getParent()).removeView(splash);
        splash = null;
        Log.i("aio", "splash: gone " + (SystemClock.uptimeMillis() - splashShown) + " ms after it showed (" + why + ")");
    }

    /** Give the page its end of a fresh channel; the app then renders into it. */
    private void wire() {
        WebMessagePort[] ports = web.createWebMessageChannel();
        if (port != null) port.close();
        port = ports[0];
        port.setWebMessageCallback(new WebMessagePort.WebMessageCallback() {
            @Override
            public void onMessage(WebMessagePort p, WebMessage m) {
                if (PAINTED.equals(m.getData())) painted();
                else Host.fromPage(m.getData());
            }
        });
        final WebMessagePort ours = port;
        Host.sink = f -> main.post(() -> {
            if (ours == port) ours.postMessage(new WebMessage(f));
        });
        web.postWebMessage(new WebMessage("aio-port", new WebMessagePort[] {ports[1]}), Uri.parse(ORIGIN));
        Host.connect();
    }

    /** A link to the app (one of its schemes): the app goes there. */
    private void link(Intent i) {
        if (i != null && Intent.ACTION_VIEW.equals(i.getAction()) && i.getDataString() != null) {
            Host.link(i.getDataString());
        }
    }

    /** A link while the app runs: singleTask brings this activity forward. */
    @Override
    protected void onNewIntent(Intent i) {
        super.onNewIntent(i);
        setIntent(i);
        link(i);
    }

    /** Does the APK declare {@code permission}? */
    private boolean declared(String permission) {
        try {
            String[] all = getPackageManager().getPackageInfo(getPackageName(), PackageManager.GET_PERMISSIONS)
                    .requestedPermissions;
            return all != null && Arrays.asList(all).contains(permission);
        } catch (PackageManager.NameNotFoundException e) {
            return false;
        }
    }

    @Override
    public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        if (code != CAMERA || pendingCamera == null) return;
        PermissionRequest r = pendingCamera;
        pendingCamera = null;
        if (results.length > 0 && results[0] == PackageManager.PERMISSION_GRANTED) r.grant(r.getResources());
        else {
            Log.w("aio", "camera DENIED to the page: the user said no");
            r.deny();
        }
    }

    /** The app's question for the system picker (Host.ask): one file, several,
     *  or where to save {@code name}. One at a time: an open one is cancelled. */
    void ask(long id, int what, String title, String[] extensions, String name) {
        if (asking >= 0) Host.answer(asking, null, null, null);
        List<String> types = new ArrayList<>();
        for (String e : extensions) {
            String t = MimeTypeMap.getSingleton().getMimeTypeFromExtension(e.toLowerCase(java.util.Locale.ROOT));
            // One unknown extension: no filter can say it, so any file.
            if (t == null) {
                types.clear();
                break;
            }
            if (!types.contains(t)) types.add(t);
        }
        Intent i = new Intent(what == 2 ? Intent.ACTION_CREATE_DOCUMENT : Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE);
        if (what == 2) {
            i.setType(types.size() == 1 ? types.get(0) : "application/octet-stream");
            i.putExtra(Intent.EXTRA_TITLE, name);
        } else {
            i.setType(types.size() == 1 ? types.get(0) : "*/*");
            if (types.size() > 1) i.putExtra(Intent.EXTRA_MIME_TYPES, types.toArray(new String[0]));
            if (what == 1) i.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        }
        try {
            startActivityForResult(i, ASK);
            asking = id;
            askingWhat = what;
        } catch (ActivityNotFoundException e) {
            Host.answer(id, null, null, "no app on this device picks a document (" + e + ")");
        }
    }

    @Override
    protected void onActivityResult(int code, int result, Intent data) {
        List<Uri> uris = new ArrayList<>();
        if (result == RESULT_OK && data != null) {
            if (data.getClipData() != null) {
                for (int n = 0; n < data.getClipData().getItemCount(); n++) uris.add(data.getClipData().getItemAt(n).getUri());
            } else if (data.getData() != null) uris.add(data.getData());
        }
        if (code == FILES) {
            if (pendingFiles != null) pendingFiles.onReceiveValue(uris.isEmpty() ? null : uris.toArray(new Uri[0]));
            pendingFiles = null;
        } else if (code == ASK && asking >= 0) {
            long id = asking;
            asking = -1;
            handOver(id, uris, askingWhat == 2 ? "wt" : "r");
        }
    }

    /** Each document's name and a descriptor the library now owns. */
    private void handOver(long id, List<Uri> uris, String mode) {
        if (uris.isEmpty()) {
            Host.answer(id, null, null, null);
            return;
        }
        String[] names = new String[uris.size()];
        int[] fds = new int[uris.size()];
        try {
            for (int n = 0; n < uris.size(); n++) {
                names[n] = "";
                try (Cursor c = getContentResolver().query(uris.get(n), new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                    if (c != null && c.moveToFirst() && c.getString(0) != null) names[n] = c.getString(0);
                }
                fds[n] = getContentResolver().openFileDescriptor(uris.get(n), mode).detachFd();
            }
        } catch (Exception e) {
            Host.answer(id, null, null, "the document could not be opened: " + e);
            return;
        }
        Host.answer(id, names, fds, null);
    }

    /** fontScale is in configChanges (a font size change reloaded the page),
     *  and the WebView takes the system's font scale only when it is made: the
     *  new scale is carried over, unless the app set a text zoom of its own. */
    @Override
    public void onConfigurationChanged(android.content.res.Configuration c) {
        super.onConfigurationChanged(c);
        if (web == null) return;
        if (c.fontScale != zoomedFontScale && web.getSettings().getTextZoom() == Math.round(zoomedFontScale * 100)) {
            web.getSettings().setTextZoom(Math.round(c.fontScale * 100));
        }
        zoomedFontScale = c.fontScale;
    }

    /** The app's Back handlers first (from a cold start too: no page script,
     *  no gesture needed); its default only when none took it. */
    @Override
    public void onBackPressed() {
        if (web == null) {
            super.onBackPressed();
            return;
        }
        Host.backDefault = () -> main.post(() -> {
            if (web != null && web.canGoBack()) web.goBack();
            else super.onBackPressed();
        });
        Host.back();
    }

    @Override
    protected void onDestroy() {
        if (Host.activity == this) Host.activity = null;
        Host.backDefault = null;
        Host.sink = null;
        if (port != null) port.close();
        if (web != null) web.destroy();
        super.onDestroy();
        // Closing the app ends it, as on the desktop: its stores finish, and
        // the next launch starts fresh.
        if (isFinishing()) {
            Host.stop();
            android.os.Process.killProcess(android.os.Process.myPid());
        }
    }
}
