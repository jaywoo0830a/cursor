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

## 사용법 / 옵션

| 옵션 | 기본값 | 설명 |
| --- | --- | --- |
| `--style` | `ring_cross` | 커서 모양: `crosshair`, `ring`, `dot`, `ring_cross`, `cross_dot` |
| `--size` | `40` | 커서 지름(px) |
| `--color` | `#FF2D55` | 커서 색상 (hex) |
| `--gap` | `0.35` | 십자 커서의 가운데 빈 간격(반지름 대비 비율) |
| `--thickness` | `2` | 선 두께(px) |
| `--fps` | `60` | 갱신 속도 |
| `--monitor` | `-1` | 표시할 모니터 인덱스(0부터), `-1`이면 전체 모니터 |
| `--hide-system-cursor` | 기본 | 시스템 커서 숨김 |
| `--show-system-cursor` | - | 시스템 커서 유지 (오버레이 위에 겹쳐 표시) |

예시:

```bat
python main.py --style crosshair --size 60 --color "#00E5FF"
python main.py --monitor 0 --fps 120
python main.py --style dot --size 16
```

## 단축키

- **Ctrl + Shift + H** : 시스템 커서 보이기/숨기기 토글

## 종료

오버레이는 시스템 트레이 아이콘이 없으므로,
터미널에서 **Ctrl + C** 를 누르거나 창을 닫아 종료합니다.

## 동작 원리

1. PySide6(Qt6)로 투명 + 프레임 없는 + 항상 위 창을 만듭니다.
2. Win32 API(ctypes)로 `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`를
   적용해 **클릭 통과** + **포커스 미탈취**가 되게 합니다.
3. `QTimer`로 포인터 위치를 폴링(`QCursor.pos`)하고, 움직였을 때만 다시 그립니다.
4. 창에 `BlankCursor`를 설정해 시스템 커서를 숨기고 커스텀 커서만 남깁니다.

## 구조

```
cursor/
├── main.py        # 진입점
├── settings.py    # 명령줄 옵션 파싱
├── cursors.py     # 커서 모양 그리기 (crosshair/ring/dot...)
├── overlay.py     # 투명 클릭 통과 오버레이 창 + 전역 단축키
├── run.bat        # Windows 실행 스크립트
└── requirements.txt
```
