"""conductor を pty 上で起動し、送ったキーごとに実画面を再構成して表示する。

build と test が緑でも「実際に描けているか」は分からない。ratatui は
カーソル移動で差分だけを書くので、出力をそのまま読んでも画面にならない。
CUP (ESC[row;colH) を再生してセルバッファを組み直すことで、フォーカス枠の
移動やツリーの展開といった見た目の変化を機械的に確認できる。

    python3 scripts/smoke-tui.py <conductor バイナリ> <対象リポジトリ>

対象リポジトリは使い捨てのクローンにすること。conductor はリポジトリにつき
1 ウィンドウのインスタンスロックを取るので、作業中のものを指すと弾かれる。
"""
import os, pty, sys, time, select, signal, re, fcntl, termios, struct

ROWS, COLS = 50, 200

class Screen:
    def __init__(self):
        self.g = [[" "]*COLS for _ in range(ROWS)]; self.r = self.c = 0
    def feed(self, s):
        i = 0
        while i < len(s):
            ch = s[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;?]*)([a-zA-Z])", s[i:])
                if m:
                    p, cmd = m.group(1), m.group(2)
                    n = [int(x) for x in p.split(";") if x.isdigit()]
                    if cmd == "H": self.r, self.c = (n[0]-1 if n else 0), (n[1]-1 if len(n) > 1 else 0)
                    elif cmd == "J" and (not n or n[0] == 2): self.g = [[" "]*COLS for _ in range(ROWS)]
                    elif cmd == "K": 
                        if 0 <= self.r < ROWS:
                            for x in range(self.c, COLS): self.g[self.r][x] = " "
                    i += m.end(); continue
                m2 = re.match(r"\x1b[\]P][^\x07\x1b]*(\x07|\x1b\\)?", s[i:])
                if m2: i += m2.end(); continue
                m3 = re.match(r"\x1b[()][B0]|\x1b[=>78]", s[i:])
                if m3: i += m3.end(); continue
                i += 1; continue
            if ch == "\n": self.r, self.c = min(self.r+1, ROWS-1), 0
            elif ch == "\r": self.c = 0
            elif ch >= " ":
                if 0 <= self.r < ROWS and 0 <= self.c < COLS: self.g[self.r][self.c] = ch
                self.c += 1
            i += 1
    def text(self): return "\n".join("".join(r).rstrip() for r in self.g)

def run(binary, repo, phases):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ.update(TERM="xterm-256color", RUST_LOG="warn")
        os.execv(binary, [binary, repo])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.set_blocking(fd, False)
    sc, raw = Screen(), b""
    def drain(sec):
        nonlocal raw
        end = time.time()+sec
        while time.time() < end:
            r,_,_ = select.select([fd],[],[],0.15)
            if r:
                try: b = os.read(fd, 1<<16)
                except OSError: return
                if not b: return
                raw += b; sc.feed(b.decode("utf-8","replace"))
    drain(4.0)
    out = [("起動直後", sc.text())]
    for label, keys in phases:
        for k in keys:
            os.write(fd, k); time.sleep(0.25)
        drain(1.5); out.append((label, sc.text()))
    try: os.kill(pid, signal.SIGKILL)
    except Exception: pass
    return out, raw.decode("utf-8","replace")

if __name__ == "__main__":
    phases = [("Tab でフォーカス移動", [b"\t"]), ("もう一度 Tab", [b"\t"]), ("F10 メニュー", [b"\x1b[21~"]), ("Esc", [b"\x1b"])]
    shots, raw = run(sys.argv[1], sys.argv[2], phases)
    bad = [l for l in raw.splitlines() if re.search(r"panicked|RUST_BACKTRACE", l)]
    print("PANIC:", bad if bad else "なし")
    for label, txt in shots:
        lines = [l for l in txt.splitlines() if l.strip()]
        print(f"\n===== {label} — 非空行 {len(lines)} =====")
        for l in lines[:14]: print("  |", l[:190])
