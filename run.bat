@echo off
REM Launch the custom cursor overlay (Windows only).
REM Edit the style/size/color below to your liking.
cd /d "%~dp0"

where python >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Python was not found. Install Python 3.10+ from https://python.org
    pause
    exit /b 1
)

echo Installing dependencies...
python -m pip install -r requirements.txt

echo Starting overlay...
python main.py --style ring_cross --size 40 --color "#FF2D55" --hide-system-cursor
pause
