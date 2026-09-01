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

오버레이는 전체 화면을 덮는 최상위 창으로 **커서를 소유**해서 어느 앱 창 위에서도
시스템/펜 커서를 숨깁니다. 마우스/펜으로 누르는 순간 잠깐 클릭 통과로 바뀌어
아래 앱이 입력(펜 압력 포함)을 그대로 받고, 버튼을 놓으면 다시 커서를 숨깁니다.

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
| `--click-through` | - | 오버레이를 항상 클릭 통과로 유지 (구버전 동작). 이 모드에선 아래 앱 창 위의 시스템 커서를 숨길 수 없음 |
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

펜 입력은 마우스와 다른 경로(`WM_POINTER*`)로 들어오고, Windows는 커서 표시를
스레드 단위로 제어합니다. 그래서 클릭 통과 창으로는 아래 앱 위의 시스템 커서를
숨길 수 없습니다. 이 프로그램은 기본적으로 **커서를 소유**하는 방식으로 해결합니다.

- **커서 숨기기 (전역)**: 오버레이는 최상위 + 히트 가능 창이라 어느 창 위에서든
  시스템/펜 커서를 소유합니다. `WM_SETCURSOR`에 `SetCursor(NULL)`로 응답해
  마우스·펜 커서를 전역으로 숨깁니다. (추가로 `PenVisualization=0` 레지스트리로
  Windows Ink 펜 커서도 비활성화하며, 종료 시 원복됩니다.)
- **입력은 그대로 전달**: 마우스/펜 버튼을 누르는 순간 오버레이가 잠깐 클릭 통과로
  바뀌고 눌림을 재주입합니다. 이후 스트로크는 아래 앱이 실제 입력으로 받아서
  **펜 압력이 유지**됩니다. 버튼을 놓으면 다시 커서를 숨깁니다.
- **위치 추적**: `QCursor.pos()`를 200Hz로 폴링해 커스텀 커서를 그립니다.
- **호환 모드**: `--click-through`를 주면 구버전처럼 항상 클릭 통과로 동작하지만,
  이 경우 아래 앱 창 위의 시스템 커서는 숨길 수 없습니다(Windows 제약).

## 종료

오버레이는 시스템 트레이 아이콘이 없으므로,
터미널에서 **Ctrl + C** 를 누르거나 창을 닫아 종료합니다.

## 동작 원리

1. PySide6(Qt6)로 투명 + 프레임 없는 + 항상 위 창을 만듭니다.
2. Win32 API(ctypes)로 `WS_EX_LAYERED | WS_EX_NOACTIVATE`를 적용해
   **포커스 미탈취** + 투명 렌더링을 합니다. 기본 모드에선 `WS_EX_TRANSPARENT`를
   쓰지 않아 오버레이가 **커서를 소유**합니다.
3. `QTimer`로 포인터 위치를 폴링(`QCursor.pos`, 기본 200Hz)하고,
   움직였을 때만 커스텀 커서를 다시 그립니다.
4. 오버레이가 최상위 + 히트 가능 창이므로 어느 창 위에서든 `WM_SETCURSOR`를 받아
   `SetCursor(NULL)` + `TRUE`로 응답 → 시스템/펜 커서를 **전역으로 숨깁니다**.
5. 버튼을 누르면(마우스/펜) `WS_EX_TRANSPARENT`로 잠깐 전환하고 눌림을 재주입해
   아래 앱이 실제 입력(펜 압력 포함)을 받게 합니다. 버튼을 놓으면
   `GetAsyncKeyState` 폴링으로 다시 커서 소유 상태로 복귀합니다.
6. 종료 시 펜 커서 레지스트리(`PenVisualization`) 원복 + 커서 표시 복구.

## 구조

```
cursor/
├── main.py          # 진입점
├── settings.py      # 명령줄 옵션 파싱
├── cursors.py       # 커서 모양 그리기 (crosshair/ring/dot...)
├── win_cursor.py    # 시스템/펜 커서 전역 숨김 보조 (ShowCursor + 레지스트리)
├── input_forward.py # 커서 소유 모드 입력 처리 (클릭 통과 전환 + 재주입)
├── overlay.py       # 투명 오버레이 창 + 전역 커서 숨김 + 전역 단축키
├── run.bat          # Windows 실행 스크립트
└── requirements.txt
```
