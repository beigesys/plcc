# Water Treatment Plant — FUXA Dashboard Layout

```
┌─────────────────────────────────────────────────────────────────────────┐
│  WATER TREATMENT PLANT CONTROL                                          │
│                                                                         │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐     │
│  │▶ START   │ │⏹ STOP    │ │🔄 FLUSH  │ │⚠ ACK    │ │🔧 RESET  │     │
│  │(cmd_start)│ │(cmd_stop) │ │(cmd_flush)│ │(cmd_ack) │ │(cmd_reset)│    │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘     │
│                                                                         │
│  STATUS: ● Running  ○ Stopped  ○ Flushing  ○ Faulted  ● System OK     │
│                                                                         │
├─────────────────────────────────┬───────────────────────────────────────┤
│                                 │                                       │
│   RAW WATER TANK                │   CLEAN WATER TANK                    │
│   ┌─────────────┐              │   ┌─────────────┐                     │
│   │             │              │   │             │                     │
│   │    ████     │ Level: 50%   │   │   ██████    │ Level: 69%          │
│   │    ████     │ SP: 50%      │   │   ██████    │ SP: 70%             │
│   │    ████     │              │   │   ██████    │                     │
│   │    ████     │              │   │   ██████    │                     │
│   │             │              │   │   ██████    │                     │
│   └─────────────┘              │   └─────────────┘                     │
│                                 │                                       │
├─────────────────────────────────┴───────────────────────────────────────┤
│                                                                         │
│   PUMPS                                                                 │
│   ┌───────────────────┐ ┌───────────────────┐ ┌───────────────────┐    │
│   │ INLET PUMP        │ │ TRANSFER PUMP     │ │ OUTLET PUMP       │    │
│   │ ● Running         │ │ ● Running         │ │ ● Running         │    │
│   │ Speed: 60%        │ │ Speed: 40%        │ │ Speed: 30%        │    │
│   │ ○ Fault           │ │ ○ Fault           │ │ ○ Fault           │    │
│   └───────────────────┘ └───────────────────┘ └───────────────────┘    │
│                                                                         │
│   VALVES                                                                │
│   Inlet: [████████░░] 100%    Outlet: [████████░░] 100%                │
│   Drain: [░░░░░░░░░░]   0%                                             │
│                                                                         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│   ALARMS                              │  COUNTERS                       │
│   ○ Raw Tank HIGH                     │  Cycle Count: 14565             │
│   ● Raw Tank LOW                      │  Total Flow:  28402             │
│   ○ Clean Tank HIGH                   │  Alarm Word:  0x0002            │
│   ○ Clean Tank LOW                    │                                 │
│   ○ Inlet Pump Fault                  │                                 │
│   ○ Transfer Pump Fault               │                                 │
│   ○ Outlet Pump Fault                 │                                 │
│                                        │                                │
└─────────────────────────────────────────────────────────────────────────┘
```

## Widget → Tag Mapping

### Command Buttons (write coil = 1 on click)

| Widget | Tag | Coil Address | Color |
|--------|-----|-------------|-------|
| START button | cmd_start | 0 | Green #4CAF50 |
| STOP button | cmd_stop | 1 | Red #f44336 |
| FLUSH button | cmd_flush | 2 | Orange #FF9800 |
| ACK button | cmd_alarm_ack | 3 | Gray #9E9E9E |
| RESET button | cmd_alarm_reset | 4 | Gray #607D8B |

### Status LEDs (read coil, green=on)

| Widget | Tag | Coil Address | ON color |
|--------|-----|-------------|----------|
| Running LED | running | 10 | Green |
| Stopped LED | stopped | 11 | Red |
| Flushing LED | flushing | 12 | Orange |
| Faulted LED | faulted | 13 | Red |
| System OK LED | system_ok | 17 | Green |

### Tank Levels (progress bar or gauge, read holding register)

| Widget | Tag | Register | Range |
|--------|-----|---------|-------|
| Raw tank level | raw_level | 16 | 0-100% |
| Raw tank setpoint | sp_raw_tank | 6 | 0-100% |
| Clean tank level | clean_level | 17 | 0-100% |
| Clean tank setpoint | sp_clean_tank | 7 | 0-100% |

### Pump Displays (LED + value)

| Widget | Tag | Address | Type |
|--------|-----|---------|------|
| Inlet run LED | inlet_pump_run | coil 20 | Bool |
| Inlet speed | inlet_pump_speed | reg 22 | UInt16 |
| Inlet fault LED | inlet_pump_fault | coil 22 | Bool |
| Transfer run LED | xfer_pump_run | coil 23 | Bool |
| Transfer speed | xfer_pump_speed | reg 25 | UInt16 |
| Transfer fault LED | xfer_pump_fault | coil 25 | Bool |
| Outlet run LED | outlet_pump_run | coil 26 | Bool |
| Outlet speed | outlet_pump_speed | reg 28 | UInt16 |
| Outlet fault LED | outlet_pump_fault | coil 28 | Bool |

### Valve Displays (progress bar, read holding register)

| Widget | Tag | Register | Range |
|--------|-----|---------|-------|
| Inlet valve | inlet_vlv_pos | 30 | 0-100% |
| Outlet valve | outlet_vlv_pos | 31 | 0-100% |
| Drain valve | drain_vlv_pos | 32 | 0-100% |

### Alarm LEDs (read coil, red=on)

| Widget | Tag | Coil Address |
|--------|-----|-------------|
| Raw HIGH | alm_raw_high | 32 |
| Raw LOW | alm_raw_low | 33 |
| Clean HIGH | alm_clean_high | 34 |
| Clean LOW | alm_clean_low | 35 |
| Inlet fault | alm_inlet_fault | 36 |
| Transfer fault | alm_xfer_fault | 37 |
| Outlet fault | alm_outlet_fault | 38 |

### Counters (value display, read holding register)

| Widget | Tag | Register |
|--------|-----|---------|
| Cycle count | cycle_count | 15 |
| Total flow | total_flow | 20 |
| Alarm count | alm_count | 40 |
| Alarm word | alarm_word | 19 |

## FUXA Editor Tips

1. **Buttons**: Drag "Button" → double-click → Events → click → SetValue → pick tag → value = 1
2. **LEDs**: Drag "Circle" → Animations → Shapes → color → bind to bool tag
3. **Gauges**: Drag "Progress" → bind variable → set min=0 max=100
4. **Values**: Drag "Value" → bind variable → shows number
5. **Labels**: Drag "Text" → type the label name
