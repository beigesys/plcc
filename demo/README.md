# plcc Demo: Water Treatment PLC + FUXA SCADA

Self-contained demo running a compiled IEC 61131-3 Structured Text program
with a SCADA dashboard — all in Docker.

## Quick Start

```bash
docker compose up -d
```

Then open **http://localhost:1881** for the FUXA SCADA dashboard.

The PLC simulator runs the water treatment plant program and exposes
Modbus TCP on **port 1502**.

## Architecture

```
┌─────────────────────────────┐
│  FUXA SCADA (Docker)        │
│  http://localhost:1881      │
│  Reads/writes Modbus TCP    │────── Modbus TCP ──────┐
└─────────────────────────────┘                        │
                                                       ▼
┌─────────────────────────────┐    ┌──────────────────────────────┐
│  ST Source (st-programs/)   │───▶│  plcc PLC Sim (Docker)     │
│  water_treatment.st         │    │  JIT-compiled, 100Hz scan    │
│  batch_process.st           │    │  Modbus TCP on :1502         │
│  motor_control.st           │    └──────────────────────────────┘
└─────────────────────────────┘
```

## FUXA Setup

1. Open http://localhost:1881
2. Go to **Setup** (gear icon) → **Devices**
3. Click **+** to add a device
4. Select **Modbus TCP**
5. Host: `localhost`, Port: `1502`
6. Add tags with these register addresses:

### Register Map (Water Treatment)

| Register | Name | Type | Access |
|----------|------|------|--------|
| 0 | mode | INT | R/W (0=Stop, 1=Run, 2=Flush) |
| 1 | raw_tank_sp | INT | R/W (setpoint %) |
| 2 | clean_tank_sp | INT | R/W (setpoint %) |
| 3 | inlet_speed | INT | R/W (pump speed %) |
| 4 | transfer_speed | INT | R/W (%) |
| 5 | outlet_speed | INT | R/W (%) |

Higher registers contain process values (cycle count, tank levels, pump states, etc.)

## Changing the Program

Edit files in `st-programs/` and restart:

```bash
docker compose restart plc
```

## Files

```
demo/
├── docker-compose.yml      # Docker Compose stack
├── Dockerfile.plc          # Builds plcc compiler + PLC sim
├── st-programs/            # ST source files (mounted as volume)
│   ├── water_treatment.st  # Main demo program
│   ├── batch_process.st
│   ├── motor_control.st
│   └── ...
├── fuxa-project/           # FUXA project configs (persisted)
└── README.md
```
