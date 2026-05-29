import asyncio
import json
import os
import sys
import time

import nodriver as uc
from nodriver import cdp


RESULT_PREFIX = "__FRANIME_RESULT__"
ERROR_PREFIX = "__FRANIME_ERROR__"
READY_PREFIX = "__FRANIME_READY__"


def write_response(obj: dict) -> None:
    sys.stdout.write(f"{RESULT_PREFIX}{json.dumps(obj)}\n")
    sys.stdout.flush()


def log(msg: str) -> None:
    sys.stderr.write(f"[py] {msg}\n")
    sys.stderr.flush()


async def read_stdin_line(loop: asyncio.AbstractEventLoop) -> str:
    return await loop.run_in_executor(None, sys.stdin.readline)


async def wait_cf_clearance(browser, tab, timeout: float = 300.0) -> tuple[str, str, dict]:
    try:
        await tab.verify_cf()
    except Exception as e:
        log(f"verify_cf: {e}")
    deadline = time.time() + timeout
    while time.time() < deadline:
        cookies = await browser.cookies.get_all()
        cf = next((c for c in cookies if c.name == "cf_clearance"), None)
        if cf and cf.value:
            try:
                ua_obj = await tab.evaluate("navigator.userAgent")
                ua = ua_obj if isinstance(ua_obj, str) else str(ua_obj)
            except Exception:
                ua = ""
            cookie_dict = {
                c.name: c.value
                for c in cookies
                if c.name.startswith("cf_") or c.name.startswith("__cf")
            }
            return cf.value, ua, cookie_dict
        await asyncio.sleep(1.5)
    raise TimeoutError("cf_clearance not obtained")


def _score_video_url(url: str, mime: str) -> int:
    score = 0
    if mime == "video/mp4":
        score += 100
    if "application/vnd.apple.mpegurl" in mime or "application/x-mpegurl" in mime:
        score += 90
    if ".m3u8" in url:
        score += 80
    if ".mp4" in url:
        score += 70
    if any(x in url for x in ("/hls", "/video/", "/stream", "playlist", "manifest")):
        score += 5
    return score


async def capture_video_url(tab, url: str, timeout: float = 25.0, referer: str = "") -> str:
    log(f"  capture: navigate {url[:90]}")
    captured: list = []

    async def on_response(event):
        try:
            r = event.response
            url_ = getattr(r, "url", "") or ""
            mime = getattr(r, "mime_type", "") or ""
            if not url_.startswith("http"):
                return
            score = _score_video_url(url_, mime)
            if score > 0:
                captured.append((score, url_, mime))
        except Exception:
            pass

    try:
        tab.add_handler(cdp.network.ResponseReceived, on_response)
        await tab.send(cdp.network.enable())
    except Exception as e:
        log(f"  capture: network handler setup failed: {e}")

    if referer:
        try:
            await tab.send(
                cdp.network.set_extra_http_headers(
                    headers=cdp.network.Headers({"Referer": referer})
                )
            )
            log(f"  capture: referer set {referer[:50]}")
        except Exception as e:
            log(f"  capture: set referer failed: {e}")

    try:
        await tab.get(url)
    except Exception as e:
        log(f"  capture tab.get failed: {e}")
        return ""

    extract_script = r"""
        (() => {
            const v = document.querySelector('video');
            if (v && v.src && v.src.startsWith('http')) return v.src;
            const s = document.querySelector('source[src]');
            if (s) {
                const sv = s.getAttribute('src') || '';
                if (sv && sv.startsWith('http')) return sv;
            }
            try {
                if (window.jwplayer) {
                    const cfg = window.jwplayer().getConfig();
                    if (cfg && cfg.sources && cfg.sources.length > 0) {
                        return cfg.sources[0].file || '';
                    }
                }
            } catch (e) {}
            const html = document.documentElement.outerHTML;
            const m = html.match(/(https?:\/\/[^"'\s<>]+\.(?:mp4|m3u8)(?:\?[^"'\s<>]*)?)/i);
            return m ? m[1] : '';
        })()
    """
    play_script = r"""
        (() => {
            const v = document.querySelector('video');
            if (v) {
                try { v.muted = true; v.play(); } catch(e) {}
                return v.src || '';
            }
            return '';
        })()
    """
    click_script = r"""
        (() => {
            const sels = ['.play', '.play-button', '.vjs-big-play-button', '[aria-label="Play"]', '#player'];
            for (const s of sels) {
                const el = document.querySelector(s);
                if (el) { try { el.click(); } catch(e) {} return s; }
            }
            return '';
        })()
    """
    deadline = time.time() + timeout
    attempts = 0
    while time.time() < deadline:
        attempts += 1
        try:
            result = await tab.evaluate(extract_script)
            if isinstance(result, str) and result and result.startswith("http"):
                log(f"  capture: js extract after {attempts}: {result[:90]}")
                return result
        except Exception as e:
            log(f"  capture eval error: {e}")
        if captured:
            captured.sort(reverse=True)
            best = captured[0]
            log(
                f"  capture: network capture after {attempts} ({best[2]}): {best[1][:90]}"
            )
            return best[1]
        if attempts == 2:
            try:
                await tab.evaluate(play_script)
            except Exception:
                pass
        if attempts == 5:
            try:
                clicked = await tab.evaluate(click_script)
                if clicked:
                    log(f"  capture: tried click on {clicked}")
            except Exception:
                pass
        await asyncio.sleep(0.5)
    if captured:
        captured.sort(reverse=True)
        best = captured[0]
        log(f"  capture: timeout, returning best from network: {best[1][:90]}")
        return best[1]
    log(f"  capture: NO url after {attempts} tries")
    return ""


async def fetch_iframe_src(tab, url: str, timeout: float = 25.0) -> str:
    log(f"  navigating to {url[:90]}")
    t0 = time.time()
    try:
        await tab.get(url)
    except Exception as e:
        log(f"  tab.get failed: {e}")
        return ""
    log(f"  navigation done in {time.time()-t0:.1f}s")

    try:
        await tab.verify_cf()
        log("  verify_cf returned")
    except Exception as e:
        log(f"  verify_cf skipped/failed: {e}")

    deadline = time.time() + timeout
    script = """
        (() => {
            const KNOWN = [
                'sibnet.ru', 'sendvid.com', 'filemoon',
                'vidmoly', 'mixdrop', 'streamtape', 'streamta.pe',
                'doodstream', 'dood.', 'upstream', 'vudeo', 'okru', 'ok.ru',
                'mp4upload', 'voe.sx', 'vk.com', 'streamwish', 'streamhide',
                'mytv', 'lulu.st', 'embedsito'
            ];
            const FORBIDDEN = ['franime.fr', 'cloudflare', 'recaptcha', 'about:blank'];
            const frames = Array.from(document.querySelectorAll('iframe'));
            const srcs = frames
                .map(i => i.src || i.getAttribute('src') || '')
                .filter(s => s && !FORBIDDEN.some(f => s.includes(f)));
            const known = srcs.filter(s => KNOWN.some(k => s.includes(k)));
            if (known.length > 0) return known[known.length - 1];
            return srcs.length > 0 ? srcs[srcs.length - 1] : '';
        })()
    """
    seen_iframes_script = """
        Array.from(document.querySelectorAll('iframe'))
            .map(i => i.src || i.getAttribute('src') || '(no src)').join(' | ')
    """
    attempts = 0
    while time.time() < deadline:
        attempts += 1
        try:
            result = await tab.evaluate(script)
            if isinstance(result, str) and result:
                log(f"  iframe found in {attempts} tries: {result[:90]}")
                return result
        except Exception as e:
            log(f"  eval error: {e}")
        if attempts == 1 or attempts == 10 or attempts == 30:
            try:
                seen = await tab.evaluate(seen_iframes_script)
                log(f"  attempt {attempts}: iframes seen = {str(seen)[:200]}")
            except Exception as e:
                log(f"  attempt {attempts}: seen-iframes eval error: {e}")
        await asyncio.sleep(0.5)
    log(f"  iframe NOT found after {attempts} tries (timeout)")
    return ""


async def main_loop() -> int:
    target_url = sys.argv[1] if len(sys.argv) > 1 else "https://franime.fr"
    headless = os.environ.get("FRANIME_HEADLESS", "0") in ("1", "true", "True")
    log(f"starting nodriver for {target_url} (headless={headless})")
    browser = await uc.start(
        headless=headless,
        browser_args=["--disable-features=IsolateOrigins,site-per-process"],
    )
    try:
        tab = await browser.get(target_url)
        log("solving CF...")
        cf_clearance, ua, cookies = await wait_cf_clearance(browser, tab)
        log(f"got cf_clearance, ua={ua[:50]}, cookies={len(cookies)}")
        write_response({
            "type": "ready",
            "cf_clearance": cf_clearance,
            "user_agent": ua,
            "all_cookies": cookies,
        })

        loop = asyncio.get_event_loop()
        while True:
            line = await read_stdin_line(loop)
            if not line:
                log("stdin closed, exiting")
                break
            line = line.strip()
            if not line:
                continue
            try:
                cmd = json.loads(line)
            except json.JSONDecodeError as e:
                log(f"bad json: {e}")
                continue

            cmd_id = cmd.get("id")
            op = cmd.get("cmd")
            if op == "fetch_iframe":
                url = cmd.get("url", "")
                log(f"fetch_iframe id={cmd_id} url={url[:80]}")
                try:
                    src = await fetch_iframe_src(tab, url)
                    write_response({"id": cmd_id, "iframe_src": src})
                except Exception as e:
                    log(f"fetch_iframe error: {e}")
                    write_response({"id": cmd_id, "error": str(e)})
            elif op == "capture_video_url":
                url = cmd.get("url", "")
                referer = cmd.get("referer", "") or ""
                log(f"capture_video_url id={cmd_id} url={url[:80]} ref={referer[:40]}")
                try:
                    src = await capture_video_url(tab, url, referer=referer)
                    write_response({"id": cmd_id, "video_url": src})
                except Exception as e:
                    log(f"capture_video_url error: {e}")
                    write_response({"id": cmd_id, "error": str(e)})
            elif op == "refresh_cf":
                log("refresh_cf requested")
                try:
                    await tab.get(target_url)
                    cf_clearance, ua, cookies = await wait_cf_clearance(browser, tab)
                    write_response({
                        "id": cmd_id,
                        "cf_clearance": cf_clearance,
                        "user_agent": ua,
                        "all_cookies": cookies,
                    })
                except Exception as e:
                    write_response({"id": cmd_id, "error": str(e)})
            elif op == "stop":
                log("stop requested")
                write_response({"id": cmd_id, "ok": True})
                break
            elif op == "ping":
                write_response({"id": cmd_id, "pong": True})
            else:
                write_response({"id": cmd_id, "error": f"unknown cmd: {op}"})
        return 0
    finally:
        try:
            browser.stop()
        except Exception:
            pass


def main() -> int:
    try:
        return asyncio.run(main_loop())
    except KeyboardInterrupt:
        return 130
    except Exception as e:
        log(f"fatal: {type(e).__name__}: {e}")
        sys.stdout.write(f"{ERROR_PREFIX}{type(e).__name__}: {e}\n")
        sys.stdout.flush()
        return 2


if __name__ == "__main__":
    sys.exit(main())
