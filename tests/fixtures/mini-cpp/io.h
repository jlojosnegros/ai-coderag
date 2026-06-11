#pragma once
#include <string>
#include <vector>

std::string read_file_to_string(const std::string& path);
void write_string_to_file(const std::string& path, const std::string& content);
std::vector<std::string> list_files_in_directory(const std::string& dir);
bool file_exists(const std::string& path);