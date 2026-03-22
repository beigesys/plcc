# plcc Demo: Water Treatment PLC on Renode + FUXA SCADA

Real PLC firmware running on an emulated STM32F4 Discovery board (Renode),
communicating via Modbus RTU to a FUXA SCADA dashboard — all in Docker.

## Quick Start

```bash
docker compose up -d
```

- **FUXA SCADA** → http://localhost:1881
- **Modbus RTU** → TCP socket on port 5020 (Renode UART bridge)
- **Debug console** → visible in `docker compose logs renode`

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│  Docker Compose                                          │
│                                                          │
│  ┌─────────────────────┐     ┌────────────────────────┐  │
│  │  Renode              │     │  FUXA SCADA            │  │
│  │  STM32F4 Discovery   │     │  http://localhost:1881 │  │
│  │                      │     │                        │  │
│  │  firmware.elf:       │     │  Modbus RTU over TCP   │  │
│  │  ├── water_treatment │◄───►│  Host: localhost       │  │
│  │  ├── modbus_rtu.c    │TCP  │  Port: 5020            │  │
│  │  └── startup_stm32f4 │:5020│                        │  │
│  │                      │     │                        │  │
│  │  USART2 ──► TCP:5020 │     └────────────────────────┘  │
│  │  USART1 ──► console  │                                 │
│  └─────────────────────┘                                  │
│                                                           │
│  water_treatment.st ──► plcc compile ──► firmware.elf   │
└──────────────────────────────────────────────────────────┘
```

## FUXA Setup

1. Open http://localhost:1881
2. **Setup** → **Devices** → **+** → **Modbus RTU over TCP**
3. Host: `localhost`, Port: `5020`, Slave ID: `1`
4. Add tags:

| Register | Name | Type | Access |
|----------|------|------|--------|
| 0 | mode | Holding | R/W (0=Stop, 1=Run, 2=Flush) |
| 1 | raw_tank_sp | Holding | R/W (setpoint %) |
| 2 | clean_tank_sp | Holding | R/W (setpoint %) |
| 3 | inlet_speed | Holding | R/W (pump speed %) |
| 4 | transfer_speed | Holding | R/W (%) |
| 5 | outlet_speed | Holding | R/W (%) |

## Rebuilding

Edit `st-programs/water_treatment.st` then:

```bash
docker compose up -d --build firmware
docker compose restart renode
```

## Files

```
demo/
├── docker-compose.yml      Docker Compose stack
├── Dockerfile.plc          Builds firmware from ST source
├── renode/
│   └── plc_demo.resc       Renode script (STM32F4 + UART bridge)
├── st-programs/            ST source files
│   ├── water_treatment.st  Main program
│   └── ...
└── README.md
```
