#include <string>
#include <cstdlib>
#include <stdexcept>

struct Config {
    int max_items;
    std::string output_path;
    bool verbose;
};

Config load_config_from_env() {
    Config cfg;
    const char* max_items_env = std::getenv("MAX_ITEMS");
    cfg.max_items = max_items_env ? std::stoi(max_items_env) : 100;
    const char* output_path_env = std::getenv("OUTPUT_PATH");
    cfg.output_path = output_path_env ? output_path_env : "./output";
    const char* verbose_env = std::getenv("VERBOSE");
    cfg.verbose = verbose_env && (std::string(verbose_env) == "1");
    return cfg;
}

Config load_config_from_map(const std::unordered_map<std::string, std::string>& map) {
    Config cfg;
    auto it = map.find("max_items");
    cfg.max_items = (it != map.end()) ? std::stoi(it->second) : 100;
    it = map.find("output_path");
    cfg.output_path = (it != map.end()) ? it->second : "./output";
    it = map.find("verbose");
    cfg.verbose = (it != map.end()) && (it->second == "true");
    return cfg;
}