package aio.rt;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;
import android.view.Window;
import android.webkit.ConsoleMessage;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import android.widget.TextView;

/** The dev APK's window (`ax dev <app> --android`): the page `ax dev --web`
 *  serves on the computer, reached as http://localhost over `adb reverse`.
 *  No library: an edit is patched into the page or reloads it, so the APK
 *  is installed once. The address comes with each start (`--es url`),
 *  kept for a start from the launcher. */
public final class DevActivity extends Activity {
    private WebView web;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        requestWindowFeature(Window.FEATURE_NO_TITLE);
        WebView.setWebContentsDebuggingEnabled(true);
        String url = getIntent().getStringExtra("url");
        android.content.SharedPreferences prefs = getSharedPreferences("aio-dev", MODE_PRIVATE);
        if (url != null) prefs.edit().putString("url", url).apply();
        else url = prefs.getString("url", null);
        if (url == null) {
            say("Start it from the computer: ax dev <app> --android");
            return;
        }
        final Uri origin = Uri.parse(url);
        web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest r) {
                Uri u = r.getUrl();
                if (u.getAuthority() != null && u.getAuthority().equals(origin.getAuthority())) return false;
                try {
                    startActivity(new Intent(Intent.ACTION_VIEW, u));
                } catch (ActivityNotFoundException e) {
                    Log.w("aio", "no app on this device opens " + u);
                }
                return true;
            }

            @Override
            public void onReceivedError(WebView v, WebResourceRequest r, WebResourceError e) {
                if (!r.isForMainFrame()) return;
                Log.e("aio", "dev page " + r.getUrl() + ": " + e.getDescription());
                say("ax dev is not serving " + origin + " (" + e.getDescription() + "): run ax dev <app> --android");
            }
        });
        web.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onConsoleMessage(ConsoleMessage m) {
                Log.i("aio", "[page] " + m.messageLevel() + ": " + m.message() + " (" + m.sourceId() + ":" + m.lineNumber() + ")");
                return true;
            }
        });
        FrameLayout frame = new FrameLayout(this);
        frame.addView(web);
        frame.setFitsSystemWindows(true);
        setContentView(frame);
        Log.i("aio", "dev page: " + url);
        web.loadUrl(url);
    }

    private void say(String text) {
        TextView t = new TextView(this);
        t.setText(text);
        t.setPadding(48, 96, 48, 48);
        setContentView(t);
    }

    @Override
    protected void onNewIntent(Intent i) {
        super.onNewIntent(i);
        setIntent(i);
        recreate();
    }

    @Override
    public void onBackPressed() {
        if (web != null && web.canGoBack()) web.goBack();
        else super.onBackPressed();
    }

    @Override
    protected void onDestroy() {
        if (web != null) web.destroy();
        super.onDestroy();
    }
}
