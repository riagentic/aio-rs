//! The manager's look (amui's `ui/style.ts`): one calm, dense system with
//! clear status colours (green up, red down or danger, blue action). amui's
//! design tokens are CSS custom properties here, its style objects classes:
//! the dark palette is amui's own, the light one its counterpart, picked by
//! the OS's theme. Every text sits on a background of the same palette,
//! never on the window's own, so no theme leaves text unreadable.

/// The window's background until the page paints, `[light, dark]`.
pub const BACKGROUND: [&str; 2] = ["#ffffff", "#0b0e14"];

/// The stylesheet: tokens, then amui's style objects as classes.
pub const CSS: &str = r#"
:root{color-scheme:dark;
--bg:#0b0e14;--bg2:#0f131b;--panel:#151a23;--panel2:#1a212c;--border:#232b38;--border-soft:#1c2430;
--text:#e6edf3;--text2:#9aa7b8;--dim:#8794ab;--green:#3fb950;--green-dim:#238636;--red:#f85149;--red-dim:#8b2c28;
--yellow:#d29922;--blue:#58a6ff;--blue-dim:#1f6feb;--purple:#bc8cff;--shadow:rgba(0,0,0,.28);
--green-bg:rgba(63,185,80,.12);--green-line:rgba(63,185,80,.3);--blue-bg:rgba(88,166,255,.10);--dim-bg:rgba(102,115,138,.12);
--mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,"DejaVu Sans Mono",monospace}
@media (prefers-color-scheme: light){:root{color-scheme:light;
--bg:#ffffff;--bg2:#f6f8fa;--panel:#ffffff;--panel2:#eef1f5;--border:#d0d7de;--border-soft:#e4e8ed;
--text:#1f2328;--text2:#424a53;--dim:#59636e;--green:#116329;--green-dim:#4ac26b;--red:#cf222e;--red-dim:#ff8182;
--yellow:#9a6700;--blue:#0550ae;--blue-dim:#218bff;--purple:#8250df;--shadow:rgba(31,35,40,.08);
--green-bg:rgba(26,127,55,.10);--green-line:rgba(26,127,55,.3);--blue-bg:rgba(9,105,218,.08);--dim-bg:rgba(89,99,110,.10)}}
*,*::before,*::after{box-sizing:border-box}
html,body,#main{margin:0;height:100%;overflow:hidden;background:var(--bg);color:var(--text)}
.am{display:flex;height:100vh;margin:0;font-family:system-ui,-apple-system,"Segoe UI",Roboto,Ubuntu,Cantarell,"Noto Sans",sans-serif;background:var(--bg);color:var(--text);overflow:hidden;font-size:13px}
.am button,.am input{font-family:inherit}
.am :focus-visible{outline:2px solid var(--blue);outline-offset:1px}
.mono{font-family:var(--mono)}
.card{background:var(--panel);border:1px solid var(--border);border-radius:10px;box-shadow:0 1px 2px var(--shadow)}
.btn{padding:7px 12px;border-radius:7px;border:1px solid var(--border);background:var(--panel2);color:var(--text);cursor:pointer;font-size:13px;transition:background .12s,border-color .12s;white-space:nowrap}
.btn:hover:not(:disabled){border-color:var(--dim)}
.btn:disabled{opacity:.55;cursor:default}
.ghost{background:transparent}
.btn.go{border-color:var(--green-dim);color:var(--green)}
.btn.act{border-color:var(--blue-dim);color:var(--blue)}
.btn.no{border-color:var(--red-dim);color:var(--red)}
.chip{display:inline-block;padding:2px 8px;border-radius:999px;font-size:11px;font-weight:600;line-height:16px;border:1px solid var(--border)}
.pill{cursor:pointer;color:var(--dim);background:var(--panel);user-select:none}
.pill.on{border-color:var(--blue);color:var(--blue);background:var(--blue-bg)}
.label{color:var(--dim);font-size:10px;font-weight:700;letter-spacing:.06em;text-transform:uppercase}
.dim{color:var(--dim)}.text2{color:var(--text2)}.green{color:var(--green)}.red{color:var(--red)}
.yellow{color:var(--yellow)}.blue{color:var(--blue)}.purple{color:var(--purple)}
.ell{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.field{padding:7px 10px;border-radius:7px;border:1px solid var(--border);background:var(--panel);color:var(--text);font-size:13px;min-width:0}
.field::placeholder{color:var(--dim)}
.link{color:var(--blue);cursor:pointer}
/* sidebar */
.side{width:280px;min-width:280px;border-right:1px solid var(--border);display:flex;flex-direction:column;background:var(--bg2)}
.brand{padding:16px 16px 12px}.brand b{display:block;font-weight:800;font-size:17px;letter-spacing:.02em}.brand span{color:var(--dim);font-size:11px}
.side-tools{padding:0 12px 10px;display:flex;flex-direction:column;gap:8px}
.row6{display:flex;gap:6px}
.side-list{overflow-y:auto;flex:1;padding:4px 8px}
.proj{padding:9px 10px;border-radius:8px;cursor:pointer;margin-bottom:2px;border:1px solid transparent;display:flex;align-items:center;gap:8px}
.proj:hover{background:var(--panel)}
.proj.sel{background:var(--panel2);border-color:var(--border)}
.dot{width:8px;height:8px;border-radius:999px;background:var(--dim);flex-shrink:0}.dot.up{background:var(--green)}
.proj b{display:block;font-weight:600;font-size:13px}
.proj small{display:block;color:var(--dim);font-size:10px}
.side-foot{padding:8px 14px;border-top:1px solid var(--border);color:var(--dim);font-size:10px;font-family:var(--mono)}
.note{padding:12px;color:var(--dim);font-size:12px}
/* detail */
.detail{flex:1;display:flex;flex-direction:column;min-width:0}
.head{padding:18px 24px 0;border-bottom:1px solid var(--border);background:var(--bg2)}
.head-top{display:flex;align-items:flex-start;justify-content:space-between;gap:12px}
.title{display:flex;align-items:center;gap:10px}.title b{font-size:22px;font-weight:800}
.path{color:var(--dim);font-size:12px;font-family:var(--mono);margin-top:3px;word-break:break-all}
.acts{display:flex;gap:8px;flex-shrink:0}
.confirm{display:flex;align-items:center;gap:8px;padding:8px 0 0;font-size:12px;color:var(--text)}
.msg{color:var(--text2);font-size:12px;padding:6px 0 0;white-space:pre-wrap}
.banner{margin-top:8px;padding:8px 10px;border:1px solid var(--red-dim);border-radius:4px;color:var(--red);font-size:12px;white-space:pre-wrap}
.tabs{display:flex;gap:2px;margin-top:12px;overflow-x:auto}
.tab{padding:8px 14px;border:none;border-bottom:2px solid transparent;background:transparent;color:var(--dim);cursor:pointer;font-size:13px;font-weight:500;white-space:nowrap}
.tab.on{border-bottom-color:var(--blue);color:var(--text);font-weight:600}
.body{padding:20px 24px;overflow-y:auto;flex:1}
.empty{color:var(--dim);padding:20px;font-size:13px}
.pick{flex:1;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:8px;color:var(--dim)}
.col{display:flex;flex-direction:column;gap:14px}.col10{display:flex;flex-direction:column;gap:10px}
.wrap{display:flex;gap:14px;flex-wrap:wrap}.wrap10{display:flex;gap:10px;flex-wrap:wrap}
.box{padding:14px 16px;flex:1;min-width:260px}
.tile{padding:10px 14px;flex:1 1 0;min-width:max-content}
.tile .v{font-size:19px;font-weight:700;font-family:var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.tile .s{font-size:11px;color:var(--dim)}
.kv{display:flex;padding:3px 0;font-size:13px}.kv span:first-child{color:var(--dim);width:80px;flex-shrink:0}.kv span:last-child{word-break:break-word}
.hrow{display:flex;align-items:center;gap:8px;padding:3px 0;font-size:13px}
.mb8{margin-bottom:8px}.mb6{margin-bottom:6px}.mb10{margin-bottom:10px}
.small{font-family:var(--mono);font-size:11px}
/* cells */
.cellcard{padding:12px 14px;margin-bottom:8px}
.cellname{font-weight:700;color:var(--blue);min-width:120px;display:flex;align-items:center;gap:8px}
.mini{padding:2px 7px;font-size:11px;font-weight:500;color:var(--dim)}
.run{padding:4px 10px;font-size:12px}
.args{flex:1;font-family:var(--mono);font-size:11px;padding:3px 6px;background:var(--bg);color:var(--text);border:1px solid var(--border);border-radius:3px}
/* tasks, files */
.out{margin:0;padding:12px 16px;font-family:var(--mono);font-size:12px;color:var(--text2);white-space:pre-wrap;word-break:break-word;max-height:420px;overflow:auto;background:var(--bg)}
.outhead{padding:10px 16px;border-bottom:1px solid var(--border);display:flex;justify-content:space-between}
.files{display:flex;gap:14px;height:100%;min-height:300px}
.tree{padding:0;width:320px;min-width:320px;display:flex;flex-direction:column;max-height:72vh}
.tree-tools{padding:8px;border-bottom:1px solid var(--border);display:flex;flex-direction:column;gap:6px}
.node{padding:3px 6px;font-family:var(--mono);font-size:12px;cursor:pointer;border-radius:5px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:var(--text)}
.node.dir{color:var(--text2)}.node.root{font-weight:700;color:var(--text2)}.node.open{color:var(--blue);background:var(--panel2)}
.node:hover{background:var(--panel2)}
.viewer{flex:1;min-width:0;overflow:hidden;display:flex;flex-direction:column;max-height:70vh}
.code{margin:0;padding:12px 14px;font-family:var(--mono);font-size:12px;color:var(--text2);white-space:pre;overflow:auto;flex:1;background:var(--bg)}
.hl-comment{color:var(--dim);font-style:italic}.hl-string{color:var(--green)}.hl-number,.hl-key{color:var(--blue)}.hl-keyword{color:var(--purple)}
/* state */
.legend{display:flex;gap:10px;align-items:center;font-size:11px;color:var(--dim)}
.cellstate{border:1px solid var(--border);border-radius:10px;overflow:hidden;background:var(--panel)}
.cellstate-head{display:flex;align-items:baseline;justify-content:space-between;padding:8px 12px;background:var(--panel2)}
.frow{display:flex;align-items:center;gap:8px;padding:5px 10px;font-family:var(--mono);font-size:12px}
.frow.obj{cursor:pointer}
.badge{font-family:var(--mono);font-size:9px;line-height:14px;padding:0 5px;border-radius:4px;border:1px solid var(--border);color:var(--dim);opacity:.45;text-decoration:line-through;user-select:none}
.badge.disk.on{border-color:var(--green);color:var(--green);background:var(--green-bg);opacity:1;text-decoration:none}
.badge.ui.on{border-color:var(--blue);color:var(--blue);background:var(--blue-bg);opacity:1;text-decoration:none}
.bar{display:inline-block;width:64px;height:4px;border-radius:2px;background:var(--panel2);overflow:hidden}
.bar>span{display:block;height:100%;background:var(--blue-dim)}
.jrow{font-family:var(--mono);font-size:12px;line-height:20px;white-space:nowrap}
/* metrics */
.chart{background:var(--panel);border:1px solid var(--border);border-radius:10px;padding:12px 14px;flex:1;min-width:0}
.chart svg{width:100%;height:70px;margin-top:6px;display:block}
.mcard{padding:12px 14px;flex:1;min-width:280px}
.stat{display:flex;justify-content:space-between;padding:3px 0;font-size:12px;font-family:var(--mono)}
.stat span:first-child{color:var(--dim)}.stat span:last-child{color:var(--text2);text-align:right}
.gauge{height:5px;border-radius:3px;background:var(--panel2);overflow:hidden}
.tbl{width:100%;border-collapse:collapse;font-family:var(--mono);font-size:11px}
.tbl th{color:var(--dim);text-align:left;padding:3px 8px;font-weight:600}
.tbl td{padding:3px 8px;border-top:1px solid var(--border-soft);color:var(--text2)}
.act-row{display:flex;justify-content:space-between;gap:8px;padding:2px 0;font-size:11px;font-family:var(--mono);border-bottom:1px solid var(--border-soft)}
/* logs */
.logbox{background:var(--bg);border:1px solid var(--border);border-radius:8px;padding:8px 10px;max-height:68vh;overflow:auto;font-family:var(--mono);font-size:11.5px;line-height:1.5}
.line{display:flex;gap:8px;white-space:pre-wrap;word-break:break-word;padding:.5px 0}
.lvl{flex-shrink:0;width:38px;font-weight:600}
.l-error{color:var(--red)}.l-warn{color:var(--yellow)}.l-perf{color:var(--purple)}.l-info{color:var(--text2)}.l-debug,.l-trace{color:var(--dim)}
.follow.on{border-color:var(--green);color:var(--green);background:var(--green-bg)}
"#;

/// Uptime for a tile: aio-rs's one duration spelling, "—" not known yet.
pub fn uptime(secs: Option<u64>) -> String {
    secs.map_or("—".into(), |s| aio::fmt::dur(std::time::Duration::from_secs(s)))
}

/// A byte size for a tile: aio-rs's one spelling, "—" not known yet.
pub fn bytes(n: Option<u64>) -> String {
    n.map_or("—".into(), aio::fmt::bytes)
}
