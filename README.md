# Custom Cursor Overlay — Chromium(WebView2) + Rust

초경량 커스텀 커서 오버레이: **순수 CSS로 그린 커서**를 최상위 투명 창이 소유합니다.
Rust(`tao` + `wry`) 코어가 창/입력/Win32를 담당하고, 커서는 내장 Chromium(Windows의
WebView2)이 **CSS로만** 렌더링합니다. 네트워크 없이 완전 오프라인으로 동작합니다.

```
┌────────────────────────────────────────────────────────────┐
│ Rust 코어 (tao + wry)                                       │
│  · 창: 투명 · 프레임리스 · 항상 위 · 전체화면 (WebView2=Chromium)│
│  · Win32: raw input(WH_MOUSE_LL + WM_INPUT)                 │
│    대상창 찾기·추종 / 영역 판정 / 패스스루(WS_EX_TRANSPARENT)  │
│    아래 앱 입력 포워딩(PostMessage + ScreenToClient)          │
│  · IPC(JSON): 상태 → 프론트, 명령 ← 프론트                    │
└──────────────────────────┬─────────────────────────────────┘
                           │ IPC
┌──────────────────────────▼─────────────────────────────────┐
│ Chromium 프론트 (index.html — 순수 CSS/JS, 오프라인 내장)      │
│  · 커스텀 CSS 커서 (radial-gradient 링 + 섀도 + 점)            │
│  · 영역 편집기(드래그/리사이즈) + 설정 패널 + 상태바            │
└────────────────────────────────────────────────────────────┘
```

## 왜 Chromium인가? (펜 팝아웃의 근본 해결)

egui 방식은 커서를 자체 GL 표면에 *그려서* 띄우기 때문에, 펜(Windows Ink/OTD)이
터치할 때 아래 앱/드라이버가 만드는 커서와 쟁탈전이 벌어져 기본 커서가 튀어나왔습니다.

Chromium 창은 그렇지 않습니다. **영역 안에서는 이 창이 히트테스트를 소유**(비클릭통과)
하므로 `WM_SETCURSOR`를 독점하고, CSS `cursor: none`으로 OS가 이 창의 커서를 아예
그리지 않습니다. 따라서 **펜이 터치해도 기본 커서가 절대 나타나지 않고**, OS 커서
자체를 대체하므로 DirectComposition/GPU 캔버스 위에서도 항상 보입니다.

## 동작 원리 (파이프라인)

1. **캡처** — `WH_MOUSE_LL` 훅이 모든 마우스 이벤트를 전역에서 가로챕니다. 펜
   (Windows Ink/OTD)은 합성 마우스 메시지를 내보내므로 훅이 함께 잡아 펜 필기도
   포워딩됩니다. 고주파 상대 델타/펜/터치는 `WM_INPUT` raw input으로 별도 캡처.
2. **판정 (Rust)** — 전역 포인터가 영역 안이면 `owning`(창이 커서 소유), 밖이면
   `passthrough`(클릭통과). 영역 경계에서 `WS_EX_TRANSPARENT`를 자동 토글.
   **펜 활성 시에는 항상 `passthrough`** — 마우스 전용 포워딩은 Windows Ink
   펜(WISP/WM_POINTER)을 재현할 수 없으므로, 펜이 쓰이는 동안은 창이 클릭통과가
   되어 아래 앱이 진짜 펜 스트로크를 직접 받습니다. (프론트가 `pointerType==='pen'`
   이벤트 또는 raw HID Pen 이벤트로 감지 → 1.2초 디케이)
3. **렌더 (Chromium)** — `owning` 상태면 프론트가 CSS 커서 div를 그립니다.
   (프론트는 네이티브 `pointermove`를 받으므로 IPC 지연 없이 커서 추적 —
   `transform`만 갱신해 레이아웃/페인트 없이 합성기에서 이동)
4. **포워딩 (Rust)** — 오버레이가 입력을 가로챘으므로 `forward_mouse`가
   `PostMessageW`(ScreenToClient 좌표 + 버튼 다운 시 SetForegroundWindow)로
   아래 창에 재생성 메시지를 보내 정상 동작하게 합니다. 프론트 UI(상태바/패널)
   위 클릭은 `set_forward_block_rects`로 차단해 아래 앱에 이중 전달되지 않습니다.

## 실행

```bash
# 대상 창 지정은 필수입니다 (전체 화면 모드는 없음).
custom-cursor-overlay --window "<window title substring>"

# 예: 제목에 "PDF"가 들어간 창 위에서만 커서 교체
custom-cursor-overlay --window "PDF"
```

빌드 (Windows):
```bash
cargo build --release
```

> Linux에서 빌드하려면 wry의 시스템 의존성인 `webkit2gtk-4.1`이 필요합니다.
> 대상 플랫폼은 Windows(WebView2 런타임 사용)입니다.

## 컨트롤

| 동작 | 효과 |
| --- | --- |
| `--window "<제목>"` | 대상 창 위에 부착 (필수) |
| 상태바 ⚙ / `F1` | 설정 패널 열기/닫기 |
| 설정 → 영역 편집 | 영역 박스 드래그 이동 / 핸들 리사이즈 |
| 설정 → 전체/중앙/초기화 | 영역 프리셋 |
| 설정 → 종료 / `Esc` | 종료 |

## 프로젝트 구조

```
src/
  main.rs              tao 이벤트 루프 + wry WebView + IPC 배선
  app.rs               상태 머신: 영역/대상창/owning·passthrough 판정
  input.rs             raw input 캡처 (WH_MOUSE_LL + WM_INPUT, ~1ms 디바운스)
  platform/
    windows.rs         GetCursorPos / 대상창 추적 / WS_EX_TRANSPARENT / 포워딩
    stub.rs            비 Windows no-op
index.html             Chromium 프론트 (CSS 커서, 영역 편집기, 설정 UI) — 완전 내장
```

## 종속성

- `tao 0.37` (창) + `wry 0.56` (WebView2) — Electron 대비 훨씬 가볍고, 시스템
  WebView2 런타임을 재사용해 설치/실행 오버헤드가 작습니다.
- `windows-sys 0.59` (raw Win32), `serde_json` (IPC), `log`/`env_logger`.
- 프론트는 CDN/외부 파일 없음 — `index.html` 하나가 바이너리에 내장됩니다.

## 알려진 한계

- 상태바 ⚙ / 설정 패널은 **영역 안에서만 클릭 가능**합니다 (영역 밖은 창이
  클릭통과라 이벤트가 안 옴).
- **펜 모드 동안에는 커스텀 커서가 숨겨집니다** — 펜이 쓰이는 동안은 클릭통과로
  전환되어 아래 앱이 진짜 Windows Ink 스트로크를 받고, 아래 앱이 잉크/자체 커서를
  그립니다 (커서보다 입력 우선).
- 빠른 **단일 탭 한 번**은 펜이 우리 창에 잡혔다가 클릭통과로 전환되는 사이 유실될
  수 있습니다 (연속 스트로크/필기는 정상).
- `PostMessage` 포워딩이라 `GetCursorPos()`를 직접 읽는 일부 앱은 실제 OS 커서
  위치와 어긋날 수 있습니다 (대부분은 메시지 좌표 `GetMessagePos`를 사용하므로
  문제없음).

