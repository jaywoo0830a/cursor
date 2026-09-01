# Custom Cursor Overlay (Windows)

Windows용 커스텀 마우스 커서를 그려주는 파이썬 오버레이입니다.
전체 화면을 덮는 투명·항상 위·클릭 통과 창이 시스템 커서를 숨기고,
실제 포인터 위치에 **링 / 십자 / 점** 형태의 커서를 그려줍니다.

드로잉 태블릿(드로잉 패드)과 함께 쓰면 펜의 절대 위치가 그대로 보여서
선을 정확하게 놓기 좋습니다.

## 설치

- Windows + Python 3.10 이상이 필요합니다.
- `run.bat`을 실행하거나 아래 명령을 직접 실행합니다.

```bat
pip install -r requirements.txt
```

## 실행

```bat
python main.py
```

기본적으로 **시스템 커서(마우스 + 펜)를 모두 숨기고** 200Hz로 갱신하며,
우리 커스텀 커서만 보이게 됩니다. `Ctrl + Shift + H`로 잠시 보이게 할 수 있습니다.

## 사용법 / 옵션

| 옵션 | 기본값 | 설명 |
| --- | --- | --- |
| `--style` | `ring_cross` | 커서 모양: `crosshair`, `ring`, `dot`, `ring_cross`, `cross_dot` |
| `--size` | `40` | 커서 지름(px) |
| `--color` | `#FF2D55` | 커서 색상 (hex) |
| `--gap` | `0.35` | 십자 커서의 가운데 빈 간격(반지름 대비 비율) |
| `--thickness` | `2` | 선 두께(px) |
| `--fps` | `200` | 갱신 속도 (200 = 고주사율 모니터 기준, 타이머 해상도 1ms 적용) |
| `--monitor` | `-1` | 표시할 모니터 인덱스(0부터), `-1`이면 전체 모니터 |
| `--hide-system-cursor` / `--no-hide-system-cursor` | 기본 켜짐 | 시스템 커서(마우스+펜) 숨기기 / 끄기 |
| `--show-system-cursor` | - | 시스템 커서 유지 (구버전 옵션, `--no-hide-system-cursor`와 동일) |
| `--no-hide-pen-cursor` | - | Windows Ink 펜 커서를 끄지 않음 (레지스트리 변경 안 함) |

예시:

```bat
python main.py --style crosshair --size 60 --color "#00E5FF"
python main.py --monitor 0 --fps 120
python main.py --style dot --size 16
```

## 단축키

- **Ctrl + Shift + H** : 시스템 커서(마우스 + 펜) 보이기/숨기기 토글

## 드로잉 패드(펜) 사용 시

펜 입력은 마우스와 다른 경로(`WM_POINTER*`)로 들어오고, 펜 호버 커서는
Windows Ink(WISP)가 별도로 그립니다. 이 프로그램은 다음과 같이 대응합니다.

- **펜 커서 숨기기**: 실행 중 `HKCU\Control Panel\Cursors\PenVisualization` 값을
  `0`으로 바꿔 펜 커서를 전역으로 끕니다. 종료하면 원래 값으로 복원됩니다.
  (Windows의 '펜 및 Windows Ink → 커서 표시' 설정과 동일한 레지스트리입니다.
  첫 적용 시 로그오프/재부팅이 필요할 수 있습니다.)
- **펜 위치 추적**: 펜이 시스템 커서를 움직이는 일반적인 설정에서는
  `QCursor.pos()`가 펜 위치를 그대로 따라가므로 커스텀 커서가 펜을 따라 움직입니다.
  드로잉 앱이 시스템 커서를 움직이지 않는(raw 펜 입력) 경우에는
  Windows 설정에서 '펜 및 Windows Ink → 커서 표시'를 켜두세요.
- **커서 되살아남 방지**: 펜/마우스를 멈춰도 Windows가 커서를 다시 표시하지 않도록
  `WM_SETCURSOR`를 가로채 `SetCursor(NULL)`을 호출하고, 주기적으로 재-숨김합니다.

## 종료

오버레이는 시스템 트레이 아이콘이 없으므로,
터미널에서 **Ctrl + C** 를 누르거나 창을 닫아 종료합니다.

## 동작 원리

1. PySide6(Qt6)로 투명 + 프레임 없는 + 항상 위 창을 만듭니다.
2. Win32 API(ctypes)로 `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`를
   적용해 **클릭 통과** + **포커스 미탈취**가 되게 합니다.
3. `QTimer`로 포인터 위치를 폴링(`QCursor.pos`)하고, 움직였을 때만 다시 그립니다.
4. 시스템 커서는 **전역**으로 숨깁니다.
   - 마우스 커서 → `ShowCursor(FALSE)` (데스크톱 전체)
   - 펜 커서 → `PenVisualization = 0` (Windows Ink 펜 호버 커서 비활성화)
5. `WM_SETCURSOR`를 가로채 `SetCursor(NULL)`을 호출하고, 타이머로 주기적으로
   재-숨김하여 Windows가 커서를 되살리는 것을 막습니다.
6. 종료 시 `ShowCursor(TRUE)` + 펜 커서 레지스트리 원복으로 원상 복구합니다.

## 구조

```
cursor/
├── main.py        # 진입점
├── settings.py    # 명령줄 옵션 파싱
├── cursors.py     # 커서 모양 그리기 (crosshair/ring/dot...)
├── win_cursor.py  # 시스템/펜 커서 전역 숨김 (ShowCursor + 레지스트리)
├── overlay.py     # 투명 클릭 통과 오버레이 창 + 전역 단축키
├── run.bat        # Windows 실행 스크립트
└── requirements.txt
```
