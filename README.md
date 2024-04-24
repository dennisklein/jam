# JAM - Job Analysis and Monitoring tool

[![license-gpl](https://alfa-ci.gsi.de/shields/badge/license-GPL--3.0--only-blue.svg)](https://spdx.org/licenses/GPL-3.0-only.html)

* **pre-alpha development**
* `-std=c++23`, requires GCC 14 (for now)
* target platform GNU/Linux, target architecture `x86_64` (for now)
* "job" as in HPC workload, e.g. an OpenMPI job scheduled via Slurm

## Dependencies

| **Dependency** | **License** | **Bundled?** |
| --- | --- | --- |
| [asio](https://think-async.com/Asio/) | [`BSL-1.0`](https://spdx.org/licenses/BSL-1.0.html) | yes |
| [Catch2](https://github.com/catchorg/Catch2) | [`BSL-1.0`](https://spdx.org/licenses/BSL-1.0.html) | yes |
| [CLI11](https://github.com/CLIUtils/CLI11) | [`BSD-3-Clause`](https://spdx.org/licenses/BSD-3-Clause.html) | yes |
| [gsl-lite](https://github.com/gsl-lite/gsl-lite) | [`MIT`](https://spdx.org/licenses/MIT.html) | yes |
| [imgui](https://github.com/ocornut/imgui) | [`MIT`](https://spdx.org/licenses/MIT.html) | yes |
| [imtui](https://github.com/ggerganov/imtui) | [`MIT`](https://spdx.org/licenses/MIT.html) | yes |
| [mp-units](https://github.com/mpusz/mp-units) | [`MIT`](https://spdx.org/licenses/MIT.html) | yes |
| [ncurses](https://invisible-island.net/ncurses/ncurses.html) | [`MIT`](https://spdx.org/licenses/MIT.html) | no |
| [simdjson](https://simdjson.org/) | [`Apache-2.0`](https://spdx.org/licenses/Apache-2.0.html) | yes |
| [slurm](https://slurm.schedmd.com/) | [`GPL-2.0-or-later`](https://spdx.org/licenses/GPL-2.0-or-later.html) | no (runtime only) |
| [systemd](https://systemd.io/) | [`LGPL-2.1-or-later`](https://spdx.org/licenses/LGPL-2.1-or-later.html) | no (runtime only) |
| [tiny-process-library](https://gitlab.com/eidheim/tiny-process-library) | [`MIT`](https://spdx.org/licenses/MIT.html) | yes |
| [Tracy](https://github.com/wolfpld/tracy) | [`BSD-3-Clause`](https://spdx.org/licenses/BSD-3-Clause.html) | yes |
