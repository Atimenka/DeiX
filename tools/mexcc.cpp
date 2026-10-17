/*
 * mexcc.cpp — Нативный драйвер кросс-компилятора C/C++ для DeiX OS (.mex v1.2)
 * Полный аналог инструментов mexc/mexmake.
 */

#include <iostream>
#include <fstream>
#include <string>
#include <vector>
#include <cstdlib>
#include <filesystem>

namespace fs = std::filesystem;

void run_cmd(const std::string& cmd) {
    std::cout << "  [mexcc-cpp] " << cmd << std::endl;
    int res = std::system(cmd.c_str());
    if (res != 0) {
        std::cerr << "ОШИБКА выполнения команды (код " << res << ")!" << std::endl;
        std::exit(1);
    }
}

fs::path locate_resource(const fs::path& exe_dir, const std::string& name) {
    std::vector<fs::path> candidates = {
        exe_dir / name,
        exe_dir / "tools" / name,
        exe_dir.parent_path() / "tools" / name,
        fs::path("tools") / name,
        fs::path("build") / name,
        fs::path(name)
    };
    for (const auto& p : candidates) {
        if (fs::exists(p)) return fs::canonical(p);
    }
    return exe_dir / name;
}

int main(int argc, char* argv[]) {
    std::cout << "DeiX C/C++ MEX Cross-Compiler Driver v1.2" << std::endl;

    if (argc < 3) {
        std::cout << "Использование: mexcc <input.cpp|c|asm> <output.mex>" << std::endl;
        return 1;
    }

    std::string input_src = argv[1];
    std::string output_mex = argv[2];

    fs::path exe_dir = fs::canonical("/proc/self/exe").parent_path();
    fs::path inc_dir = locate_resource(exe_dir, "include");
    fs::path ld_script = locate_resource(exe_dir, "mex.ld");
    fs::path pack_script = locate_resource(exe_dir, "mex_pack.py");

    std::string obj_file = "/tmp/mex_native_app.o";
    std::string elf_file = "/tmp/mex_native_app.elf";
    std::string bin_file = "/tmp/mex_native_app.bin";

    // 1. Компиляция исходного кода через g++ / gcc
    std::string compile_cmd = "g++ -c -O2 -std=c++17 -ffreestanding -fno-builtin -fno-exceptions -fno-rtti "
                              "-fno-stack-protector -mno-red-zone -Wall -I" + inc_dir.string() + " " +
                              input_src + " -o " + obj_file;
    run_cmd(compile_cmd);

    // 2. Линковка в elf_x86_64 с помощью mex.ld
    std::string link_cmd = "ld -m elf_x86_64 -T " + ld_script.string() + " " + obj_file + " -o " + elf_file;
    run_cmd(link_cmd);

    // 3. Извлечение бинарных данных
    std::string objcopy_cmd = "objcopy -O binary " + elf_file + " " + bin_file;
    run_cmd(objcopy_cmd);

    // 4. Упаковка в заголовок MEX v1.2
    std::string pack_cmd = "python3 " + pack_script.string() + " " + bin_file + " " + output_mex;
    run_cmd(pack_cmd);

    // Очистка временных файлов
    std::remove(obj_file.c_str());
    std::remove(elf_file.c_str());
    std::remove(bin_file.c_str());

    std::cout << "Успешно собрано в '" << output_mex << "'!" << std::endl;
    return 0;
}
