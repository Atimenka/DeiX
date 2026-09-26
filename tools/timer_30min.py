#!/usr/bin/env python3
"""
timer_30min.py — 30-минутный таймер выдержки и профилирования для проверки стабильности.
"""

import time
import sys

def main():
    print("=" * 60)
    print("  Запуск 30-минутного таймера стабилизации и профилирования...")
    print("=" * 60)
    
    total_seconds = 30 * 60
    interval = 60  # Вывод каждые 60 секунд (1 минута)
    
    start_time = time.time()
    
    for elapsed in range(interval, total_seconds + 1, interval):
        time.sleep(interval)
        mins = elapsed // 60
        rem = (total_seconds - elapsed) // 60
        print(f"  [ТАЙМЕР 30М] Прошло: {mins} мин. / Осталось: {rem} мин.")
        sys.stdout.flush()

    total_elapsed = time.time() - start_time
    print("=" * 60)
    print(f"  [ТАЙМЕР] 30 минут полностью истекли ({total_elapsed:.1f} сек.)!")
    print("=" * 60)

if __name__ == "__main__":
    main()
