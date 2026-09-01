# Custom Cursor Overlay — 순수 네이티브 Rust (웹뷰 없음)

최상위 투명 창이 영역 안에서 커서를 소유하고, **커서·렌더링·입력·설정 전부가 Rust +
Win32(GDI)**로 처리됩니다. **웹뷰/JS/CSS가 없습니다.** 네트워크 없이 완전 오프라인.

```
┌────────────────────────────────────────────────────────────┐
│ Rust (tao 창 + Win32) — 전부 여기                             │
│  · 커서: Rust 비트맵 → HCURSOR → SetCursor (OS가 그림)         │
│  · 창: 투명(레이어드) · 프레임리스 · 항상 위 · 전체화면 · 작업표시줄 숨김│
│  · 렌더: GDI로 영역 박스 + 상태 배지를 UpdateLayeredWindow로 그림 │
│  · 입력: WH_MOUSE_LL / WH_KEYBOARD_LL / WM_INPUT 캡처         │
│    → 아래 앱으로 100% 포워딩 (PostMessage)                     │
│  · 상호작용: 핫키 + 마우스 영역 편집 (전부 Rust)                 │
└────────────────────────────────────────────────────────────┘
```

## 왜 이 방식인가 (웹뷰 포기 이유)

- egui/Win32 네이티브 창은 투명 오버레이가 **확실히 동작**합니다. 반면 WebView2
  (Chromium)는 투명도·프레임이 환경에 따라 실패해 흰 창(Win7 스타일)으로 렌더링되는
  문제가 있었습니다 → **웹뷰 제거**.
- 커서는 **Rust가 만든 OS HCURSOR**로, 영역 안에서 창이 히트테스트를 소유
  (비클릭통과) → `WM_SETCURSOR` 독점 → 아래 앱이 절대 커서를 덮어쓸 수 없음
  (펜 팝아웃 없음, DirectComposition 위에도 보임).
- **필기**: 펜이 활성이면(raw HID in-range/down 감지) 창이 클릭통과로 전환 →
  아래 앱(OneNote 등)이 진짜 Windows Ink/WM_POINTER 스트로크를 **압력과 함께**
  직접 수신. 2초 스티키 디케이로 스트로크 중 끊김 방지.

## 입력 포워딩 (100%)

오버레이가 입력을 가로챈 동안 아래 창으로 **모든 마우스 입력을 실시간 재생성**
(`PostMessageW`):

| 입력 | 포워딩 |
| --- | --- |
| 이동 | ✅ (드래그 키 상태 + Ctrl/Shift 수정자 포함) |
| 좌/우/중간/X 버튼 | ✅ (측면 버튼 id 포함, 클릭 시 아래 창 활성화) |
| 세로 휠 | ✅ |
| **가로 휠** (좌우 스크롤) | ✅ (`WM_MOUSEHWHEEL`) |
| 터치패드 | ✅ (두 손가락 스크롤/이동이 휠·무브로 전달) |
| 펜 | ✅ (클릭통과 → 진짜 Windows Ink 수신) |

호버 효과: 포워딩 대상을 포인트 아래 **가장 깊은 자식 창**(`RealChildWindowFromPoint`)
으로 잡아 캔버스/패인의 hover도 실제 커서처럼 동작. 휠은 화면 좌표, 그 외는
클라이언트 좌표로 올바르게 변환.

## 실행

```bash
# 대상 창 지정 필수 (전체 화면 모드 없음)
custom-cursor-overlay --window "<window title substring>"
# 예: OneNote
custom-cursor-overlay --window "OneNote"
```

빌드 (Windows):
```bash
cargo build --release
```

디버그 로그:
```bash
RUST_LOG=debug cargo run -- --window "OneNote"
```

## 컨트롤 (전부 Rust 핫키 / 마우스)

| 동작 | 효과 |
| --- | --- |
| `--window "<제목>"` | 대상 창 위에 부착 (필수) |
| `Ctrl+Shift+C` | 커스텀 커서 on/off |
| `Ctrl+Shift+R` | 영역 편집 모드 (마우스 드래그 — Rust가 처리) |
| `Ctrl+Shift+O` | 영역 윤곽 표시/숨김 |
| `Ctrl+Shift+0` | 영역 = 전체 창 |
| `Ctrl+Shift+Q` / `Esc` | 종료 |

영역 편집: `Ctrl+Shift+R` 후 박스 내부 드래그 = 이동, 모서리/변 드래그 = 리사이즈.
영역 박스와 상태 배지는 **GDI로 직접 그려집니다**.

## 프로젝트 구조

```
src/
  main.rs      tao 이벤트 루프 + 네이티브 렌더 배선
  app.rs       상태 머신: 영역/대상창/owning·passthrough/핫키/영역 편집/네이티브 커서
  render.rs    GDI 레이어드 렌더링 (UpdateLayeredWindow, DIB, TextOut)
  cursor.rs    순수 Rust 커서 비트맵 생성 (링 + 점, 안티앨리어스)
  input.rs     raw input + 키보드 훅 (WH_MOUSE_LL/WH_KEYBOARD_LL + WM_INPUT)
  platform/
    windows.rs HCURSOR/SetCursor / 대상창 추적 / WS_EX_TRANSPARENT / 포워딩 / 창 폴리시
    stub.rs    비 Windows no-op
```

## 종속성

- `tao 0.37` (창) + `windows-sys 0.59` (raw Win32/GDI) + `log`/`env_logger`.
- 웹뷰/JS/CSS 없음 → 렉·투명도·호환 문제의 근원 제거.

## 알려진 한계

- **펜 모드 동안에는 커스텀 커서가 숨겨집니다** — 펜이 쓰이는 동안은 클릭통과라
  아래 앱이 잉크/자체 커서를 그립니다 (커서보다 입력 우선).
- 펜이 raw HID(Windows Ink)로 안 잡히는 일부 태블릿(OTD 등)에선 펜 감지가 늦어져
  스트로크 시작이 살짝 잘릴 수 있습니다.
- `WM_POINTER` 기반 hover를 쓰는 일부 최신 앱은 우리가 커서를 소유하는 동안 hover가
  안 될 수 있습니다 (WM_MOUSE 기반 앱은 정상).
