# Custom Cursor Overlay — Rust 중심 (tao + wry/WebView2)

초경량 커스텀 커서 오버레이: **커서는 Rust가 네이티브로 렌더링**(OS `HCURSOR` +
`SetCursor`)하고, 최상위 투명 창이 영역 안 히트테스트를 소유합니다. Chromium
(WebView2)은 **순수 CSS 프레젠테이션**(영역 박스 + 상태 줄)만 담당하고, **모든
API/IPC/입력/설정은 Rust**가 처리합니다. JS는 Rust가 보낸 값을 CSS에 적용하는
최소한만 있고, 커서는 웹뷰와 무관하므로 JS 커서 루프에 의한 렉이 없습니다.
네트워크 없이 완전 오프라인.

```
┌────────────────────────────────────────────────────────────┐
│ Rust 코어 (tao + wry + Win32) — 커서·입력·설정 전부 여기       │
│  · 커서: Rust 생성 비트맵 → HCURSOR → SetCursor (OS가 그림)   │
│    → 매끄럽고, DirectComposition 위에도, 펜에도 안전          │
│  · 창: 투명 · 프레임리스 · 항상 위 · 전체화면 · 작업표시줄 숨김  │
│  · Win32: WH_MOUSE_LL + WH_KEYBOARD_LL + WM_INPUT           │
│    핫키(Ctrl+Shift C/R/O/0/Q, Esc) / Rust 영역 편집(드래그)   │
│    대상창 추적 / 패스스루(WS_EX_TRANSPARENT) / 아래 앱 포워딩  │
│  · 상태(JSON) → 프론트 push (evaluate_script)                │
└──────────────────────────┬─────────────────────────────────┘
                           │ 상태 push (표시 전용)
┌──────────────────────────▼─────────────────────────────────┐
│ Chromium 프론트 (index.html — 순수 CSS, 오프라인 내장)         │
│  · 영역 박스(윤곽/편집) · 상태 줄(핫키 안내)                    │
│  · JS = Rust 값을 CSS에 적용만 (~15줄, API/IPC 없음)          │
│  · 커서는 그리지 않음 (owning 중엔 cursor:none, Rust가 원을 그림)│
└────────────────────────────────────────────────────────────┘
```

## 왜 이 방식이 펜 팝아웃을 해결하나

egui 방식은 커서를 자체 GL 표면에 *그려서* 띄웠기 때문에, 펜(Windows Ink/OTD)이
터치할 때 아래 앱/드라이버가 만드는 커서와 쟁탈전이 벌어져 기본 커서가 튀어나왔습니다.

이 방식은 **영역 안에서 이 창이 히트테스트를 소유**(비클릭통과) → `WM_SETCURSOR`를
독점 → 아래 앱이 절대 커서를 덮어쓸 수 없습니다. 그리고 커서는 **Rust가 만든 OS
HCURSOR**를 `SetCursor`로 지정하므로 OS가 그 창의 커서로 우리 원을 직접 그립니다
(웹뷰/JS 무관). 따라서 **펜이 터치해도 기본 커서가 절대 나타나지 않고**, OS 커서
자체를 대체하므로 DirectComposition/GPU 캔버스 위에서도 항상 보입니다.

## 동작 원리 (파이프라인)

1. **캡처 (Rust)** — `WH_MOUSE_LL` 훅이 모든 마우스 이벤트를 전역에서 가로챕니다.
   펜(Windows Ink/OTD)은 합성 마우스 메시지를 내보내므로 훅이 함께 잡아 펜 필기도
   포워딩됩니다. 고주파 상대 델타/펜/터치는 `WM_INPUT` raw input으로 별도 캡처.
2. **판정 (Rust)** — 전역 포인터가 영역 안이면 `owning`(창이 커서 소유), 밖이면
   `passthrough`(클릭통과). 영역 경계에서 `WS_EX_TRANSPARENT`를 자동 토글.
   **펜 활성 시에는 항상 `passthrough`** — 마우스 전용 포워딩은 Windows Ink
   펜(WISP/WM_POINTER)을 재현할 수 없으므로, 펜이 쓰이는 동안은 창이 클릭통과가
   되어 아래 앱이 진짜 펜 스트로크를 직접 받습니다. (raw HID Pen 이벤트로 감지,
   in-range/down 포함, 2초 스티키 디케이로 스트로크 중 끊김 방지)
3. **커서 (Rust)** — `owning` 상태면 Rust가 `SetCursor(우리 HCURSOR)`를 매 틱마다
   재단언 → OS가 우리 원을 그림. 웹뷰는 owning 중 `cursor:none`이라 간섭하지 않음.
   JS 커서 루프가 없어 렉 원인이 제거됩니다. (영역 편집 중엔 기본 화살표)
4. **포워딩 (Rust)** — 오버레이가 입력을 가로챘으므로 `forward_mouse`가
   `PostMessageW`(ScreenToClient 좌표 + 버튼 다운 시 SetForegroundWindow)로
   아래 창에 재생성 메시지를 보내 정상 동작하게 합니다. (상호작용 UI가 없으므로
   별도 차단 영역 불필요)

## 실행

```bash
# 대상 창 지정은 필수입니다 (전체 화면 모드는 없음).
custom-cursor-overlay --window "<window title substring>"

# 예: 제목에 "OneNote"가 들어간 창 위에서만 커서 교체
custom-cursor-overlay --window "OneNote"
```

빌드 (Windows):
```bash
cargo build --release
```

> Linux에서 빌드하려면 wry의 시스템 의존성인 `webkit2gtk-4.1`이 필요합니다.
> 대상 플랫폼은 Windows(WebView2 런타임 사용)입니다.

## 컨트롤 (전부 Rust — 웹뷰는 표시 전용, JS API 없음)

| 동작 | 효과 |
| --- | --- |
| `--window "<제목>"` | 대상 창 위에 부착 (필수) |
| `Ctrl+Shift+C` | 커스텀 커서 on/off |
| `Ctrl+Shift+R` | 영역 편집 모드 (마우스 드래그로 Rust가 편집) |
| `Ctrl+Shift+O` | 영역 윤곽 표시/숨김 |
| `Ctrl+Shift+0` | 영역 = 전체 창 |
| `Ctrl+Shift+Q` / `Esc` | 종료 |

영역 편집: `Ctrl+Shift+R` 켠 뒤, 박스 내부를 드래그하면 이동, 모서리/변을
드래그하면 리사이즈. **전부 Rust가 LL 훅 마우스로 처리**합니다.

## 프로젝트 구조

```
src/
  main.rs              tao 이벤트 루프 + wry WebView + 페이지 로드 시 상태 푸시
  app.rs               상태 머신 + Rust 핫키/영역 편집 + 네이티브 커서
  cursor.rs            순수 Rust 커서 비트맵 생성 (링 + 점, 안티앨리어스)
  input.rs             raw input 캡처 + 키보드 훅(핫키) (WH_MOUSE_LL/WH_KEYBOARD_LL + WM_INPUT)
  platform/
    windows.rs         HCURSOR/SetCursor / 대상창 추적 / WS_EX_TRANSPARENT / 포워딩 / 창 폴리시
    stub.rs            비 Windows no-op
index.html             순수 CSS 프레젠테이션 (영역 박스 + 상태 줄) — JS API/IPC 없음
```

## 종속성

- `tao 0.37` (창) + `wry 0.56` (WebView2) — Electron 대비 훨씬 가볍고, 시스템
  WebView2 런타임을 재사용해 설치/실행 오버헤드가 작습니다.
- `windows-sys 0.59` (raw Win32), `serde_json` (상태 직렬화), `log`/`env_logger`.
- 프론트는 CDN/외부 파일 없음 — `index.html` 하나가 바이너리에 내장됩니다.

## 알려진 한계

- **펜 모드 동안에는 커스텀 커서가 숨겨집니다** — 펜이 쓰이는 동안은 클릭통과로
  전환되어 아래 앱(OneNote 등)이 진짜 Windows Ink 스트로크를 받고, 아래 앱이
  잉크/자체 커서를 그립니다 (커서보다 입력 우선). 펜 감지는 raw HID
  in-range/down 이벤트 기반이며, 펜이 감지되면 2초간 클릭통과가 유지됩니다.
- 펜이 raw HID(Windows Ink)로 안 잡히는 일부 태블릿(OTD 등)에선 펜 감지가
  늦어져 스트로크 시작이 살짝 잘릴 수 있습니다.
- `PostMessage` 포워딩이라 `GetCursorPos()`를 직접 읽는 일부 앱은 실제 OS 커서
  위치와 어긋날 수 있습니다 (대부분은 메시지 좌표 `GetMessagePos`를 사용하므로
  문제없음).

