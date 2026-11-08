"""Host CPU / OS / compiler inspection and the harness FTZ/DAZ register probe."""

import json
import platform
import subprocess


def get_system_environment(harness_bin):
    """Inspects host hardware, OS, compiler, and hardware FTZ/DAZ register state."""
    # 1. CPU Model & Architecture
    cpu_model = ""
    sys_name = platform.system()
    if sys_name == "Linux":
        try:
            with open("/proc/cpuinfo", "r") as f:
                for line in f:
                    if line.startswith("model name"):
                        cpu_model = line.split(":", 1)[1].strip()
                        break
        except Exception:
            pass
    elif sys_name == "Darwin":
        try:
            cpu_model = subprocess.check_output(
                ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
            ).strip()
        except Exception:
            pass
    elif sys_name == "Windows":
        cpu_model = platform.processor()

    arch = platform.machine()
    cpu_desc = f"{cpu_model} ({arch})" if cpu_model else arch

    # 2. Operating System
    os_desc = f"{platform.system()} {platform.release()}"
    if sys_name == "Linux":
        try:
            os_info = platform.freedesktop_os_release()
            pretty = os_info.get("PRETTY_NAME")
            if pretty:
                os_desc = f"{pretty} (Linux {platform.release()})"
        except Exception:
            pass

    # 3. Rust Compiler Version
    rustc_ver = "Unknown"
    try:
        res = subprocess.run(["rustc", "--version"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if res.returncode == 0:
            rustc_ver = res.stdout.strip()
    except Exception:
        pass

    # 4. Harness Hardware FTZ/DAZ Register Probe
    ftz_daz_active = None
    try:
        out = subprocess.check_output([str(harness_bin), "--probe-env"], text=True).strip()
        probe = json.loads(out)
        ftz_daz_active = probe.get("ftz_daz_active")
    except Exception:
        pass

    if ftz_daz_active is True:
        ftz_status = "Active (hardware-verified)"
    elif ftz_daz_active is False:
        ftz_status = "Inactive / Not Supported"
    else:
        ftz_status = "Unknown (probe failed)"

    return {
        "cpu": cpu_desc,
        "os": os_desc,
        "rustc": rustc_ver,
        "ftz_daz_status": ftz_status,
        "ftz_daz_active": ftz_daz_active,
    }
