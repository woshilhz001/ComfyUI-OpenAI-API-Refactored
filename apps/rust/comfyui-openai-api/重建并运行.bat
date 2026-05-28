@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion

set PROJECT_ROOT=D:\ai\comfyui_2025_5\ComfyUI-OpenAI-API-Refactored
set IMAGE_NAME=comfyui-openai-api
set CONFIG_PATH=%PROJECT_ROOT%\apps\rust\comfyui-openai-api\config
set WORKFLOWS_PATH=%PROJECT_ROOT%\apps\rust\comfyui-openai-api\workflows

echo ============================================
echo  [1/4] 停止并删除所有相关容器...
echo ============================================
:: 找出所有使用该镜像的容器并强制删除（包括已停止的）
for /f "tokens=1" %%i in ('docker ps -a -q --filter "ancestor=%IMAGE_NAME%" 2^>nul') do (
    echo   删除容器: %%i
    docker rm -f %%i
)
:: 同时也按名称尝试删除（兼容旧版命名）
docker rm -f %IMAGE_NAME% 2>nul
echo   ✓ 容器清理完成

echo.
echo ============================================
echo  [2/4] 删除旧镜像（按名称 + 按 ID）...
echo ============================================
:: 先按标签删
docker rmi %IMAGE_NAME% 2>nul
:: 再找出所有同名镜像 ID 强制删
for /f "tokens=3" %%i in ('docker images %IMAGE_NAME% -q 2^>nul') do (
    echo   删除镜像: %%i
    docker rmi -f %%i 2>nul
)
echo   ✓ 镜像清理完成

echo.
echo ============================================
echo  [3/4] 构建新镜像...
echo ============================================
cd /d %PROJECT_ROOT%
docker build -t %IMAGE_NAME% -f apps/rust/comfyui-openai-api/Dockerfile .
if %errorlevel% neq 0 (
    echo   ✗ 构建失败!
    pause
    exit /b 1
)
echo   ✓ 构建完成

echo.
echo ============================================
echo  [4/4] 启动容器...
echo ============================================
docker run -d -p 8080:8080 ^
  -v "%CONFIG_PATH%:/app/config" ^
  -v "%WORKFLOWS_PATH%:/app/workflows" ^
  --name %IMAGE_NAME% ^
  %IMAGE_NAME%

if %errorlevel% neq 0 (
    echo   ✗ 容器启动失败!
    pause
    exit /b 1
)
echo   ✓ 容器已启动

echo.
echo ============================================
echo   全部完成! 容器运行中: http://localhost:8080
echo ============================================
pause
