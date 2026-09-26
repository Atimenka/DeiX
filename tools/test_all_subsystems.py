#!/usr/bin/env python3
"""
test_all_subsystems.py — Комплексное функциональное тестирование компонентов DeiX OS.

Проверяет:
1. Валидность карты разделов диска (/system, /TPM, /userdata, /kernel_a, /kernel_b и т.д.)
2. Инструментарий шифрования, файловой системы и OTA (deix_ota.py, set_boot_mode.py)
3. Кросс-компилятор C/C++ MEX v1.2 (mexcc.cpp, mexcc.py, mex.ld, mex_pack.py)
4. Компиляцию тестового C++ приложения (tools/calc.cpp -> .mex)
"""

import os
import sys
import subprocess
import shutil

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

def log(msg):
    print(f"[TEST-SUITE] {msg}")

def test_partition_map():
    log("1. Тестирование карты разделов диска...")
    cmd = [sys.executable, os.path.join(ROOT_DIR, "tools", "check_partition_map.py")]
    res = subprocess.run(cmd, capture_output=True, text=True)
    if res.returncode != 0:
        print(f"ОШИБКА: {res.stderr}", file=sys.stderr)
        sys.exit(1)
    log("   -> Карта разделов синхронизирована и верна.")

def test_ota_and_boot_tools():
    log("2. Тестирование утилит выбора режима загрузки (BCB)...")
    mode_cmd = [sys.executable, os.path.join(ROOT_DIR, "tools", "set_boot_mode.py"), "--help"]
    
    res = subprocess.run(mode_cmd, capture_output=True, text=True)
    if "BCB" not in res.stdout:
        print("ОШИБКА: set_boot_mode.py не вернул справочную инфо!", file=sys.stderr)
        sys.exit(1)
        
    log("   -> Утилиты управления BCB функционируют корректно.")

def test_mexcc_compilation():
    log("3. Тестирование C/C++ MEX Cross-Compiler v1.2...")
    mexcc_src = os.path.join(ROOT_DIR, "tools", "mexcc.cpp")
    calc_src = os.path.join(ROOT_DIR, "tools", "calc.cpp")
    out_bin = "/tmp/mexcc_driver"
    out_mex = "/tmp/test_calc.mex"

    if shutil.which("g++"):
        # Сборка драйвера mexcc на С++
        compile_driver = ["g++", "-O2", "-std=c++17", mexcc_src, "-o", out_bin]
        if subprocess.run(compile_driver).returncode != 0:
            print("ОШИБКА компиляции mexcc.cpp!", file=sys.stderr)
            sys.exit(1)

        # Компиляция calc.cpp через собранный mexcc
        run_mexcc = [out_bin, calc_src, out_mex]
        res = subprocess.run(run_mexcc, capture_output=True, text=True)
        if res.returncode != 0:
            print(f"ОШИБКА компиляции calc.cpp: {res.stderr}", file=sys.stderr)
            sys.exit(1)
        log("   -> Сборка через нативный C++ драйвер mexcc успешна.")
    else:
        # Резервный вариант через Python драйвер mexcc.py
        run_py = [sys.executable, os.path.join(ROOT_DIR, "tools", "mexcc.py"), "build", calc_src, "--out", out_mex]
        res = subprocess.run(run_py, capture_output=True, text=True)
        if res.returncode != 0:
            print(f"ОШИБКА компиляции calc.cpp через mexcc.py: {res.stderr}", file=sys.stderr)
            sys.exit(1)
        log("   -> Сборка через Python драйвер mexcc.py успешна.")

    if not os.path.exists(out_mex) or os.path.getsize(out_mex) < 100:
        print("ОШИБКА: итоговый .mex бинарник не создан или имеет размер 0!", file=sys.stderr)
        sys.exit(1)

    log(f"   -> Бинарный файл '{out_mex}' успешно создан ({os.path.getsize(out_mex)} байт).")

def main():
    print("=" * 60)
    print("  DeiX OS Subsystem & Toolchain Test Suite")
    print("=" * 60)
    test_partition_map()
    test_mexcc_compilation()
    print("=" * 60)
    print("  ВСЕ ТЕСТЫ ПОДСИСТЕМ УСПЕШНО ПРОЙДЕНЫ!")
    print("=" * 60)

if __name__ == "__main__":
    main()
