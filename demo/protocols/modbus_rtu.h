// SPDX-License-Identifier: MPL-2.0
// Modbus RTU Slave — minimal bare-metal implementation
// Supports FC01 (read coils), FC03 (read holding registers),
// FC06 (write single register), FC16 (write multiple registers)
//
// No dynamic allocation, no OS dependencies.

#ifndef MODBUS_RTU_H
#define MODBUS_RTU_H

#include <stdint.h>

// ── Modbus function codes ──

#define MODBUS_FC_READ_COILS            0x01
#define MODBUS_FC_READ_HOLDING_REGS     0x03
#define MODBUS_FC_WRITE_SINGLE_REG      0x06
#define MODBUS_FC_WRITE_MULTIPLE_REGS   0x10

// ── Modbus exception codes ──

#define MODBUS_EX_ILLEGAL_FUNCTION      0x01
#define MODBUS_EX_ILLEGAL_DATA_ADDRESS  0x02
#define MODBUS_EX_ILLEGAL_DATA_VALUE    0x03
#define MODBUS_EX_SLAVE_DEVICE_FAILURE  0x04

// ── Configuration ──

#define MODBUS_MAX_PDU_SIZE     256
#define MODBUS_MAX_REGS         128

// ── Register map for WaterTreatment program ──
// Holding Registers (FC03/FC06/FC16 read/write):
//   0  = mode (0=stop, 1=run, 2=flush)
//   1  = raw_tank_sp (setpoint %)
//   2  = clean_tank_sp (setpoint %)
//   3  = inlet_speed
//   4  = transfer_speed
//   5  = outlet_speed
//
// Input Registers (FC03 read-only, starting at address 100):
//   100 = cycle_count
//   101 = raw_tank level_pct
//   102 = clean_tank level_pct
//   103 = inlet_pump running (0/1)
//   104 = transfer_pump running (0/1)
//   105 = outlet_pump running (0/1)
//   106 = system_ok (0/1)
//   107 = alarm_count
//   108 = total_flow

#define REG_MODE            0
#define REG_RAW_TANK_SP     1
#define REG_CLEAN_TANK_SP   2
#define REG_INLET_SPEED     3
#define REG_TRANSFER_SPEED  4
#define REG_OUTLET_SPEED    5
#define REG_HOLDING_COUNT   6

#define REG_CYCLE_COUNT        100
#define REG_RAW_LEVEL_PCT      101
#define REG_CLEAN_LEVEL_PCT    102
#define REG_INLET_RUNNING      103
#define REG_TRANSFER_RUNNING   104
#define REG_OUTLET_RUNNING     105
#define REG_SYSTEM_OK          106
#define REG_ALARM_COUNT        107
#define REG_TOTAL_FLOW         108
#define REG_INPUT_FIRST        100
#define REG_INPUT_LAST         108
#define REG_INPUT_COUNT        9

// ── Callback types ──

// Read a register value. Returns 0 on success, -1 if address invalid.
typedef int (*modbus_read_reg_fn)(uint16_t addr, uint16_t *value, void *ctx);

// Write a register value. Returns 0 on success, -1 if address invalid or read-only.
typedef int (*modbus_write_reg_fn)(uint16_t addr, uint16_t value, void *ctx);

// Read a coil (single bit). Returns 0 on success, -1 if address invalid.
typedef int (*modbus_read_coil_fn)(uint16_t addr, uint8_t *value, void *ctx);

// ── Modbus RTU context ──

typedef struct {
    uint8_t  slave_addr;              // This slave's address (1-247)
    uint8_t  rx_buf[MODBUS_MAX_PDU_SIZE];
    uint16_t rx_len;
    uint8_t  tx_buf[MODBUS_MAX_PDU_SIZE];
    uint16_t tx_len;

    // Callbacks
    modbus_read_reg_fn   read_reg;
    modbus_write_reg_fn  write_reg;
    modbus_read_coil_fn  read_coil;
    void                *user_ctx;

    // Statistics
    uint32_t rx_count;
    uint32_t tx_count;
    uint32_t err_count;
} modbus_rtu_t;

// ── API ──

// Initialize a Modbus RTU slave context.
void modbus_rtu_init(modbus_rtu_t *ctx, uint8_t slave_addr,
                     modbus_read_reg_fn read_reg,
                     modbus_write_reg_fn write_reg,
                     modbus_read_coil_fn read_coil,
                     void *user_ctx);

// Feed a received byte into the Modbus RTU state machine.
// Call this from the UART RX interrupt or poll loop.
void modbus_rtu_rx_byte(modbus_rtu_t *ctx, uint8_t byte);

// Process a complete frame in rx_buf. Call after detecting end-of-frame
// (3.5 character silence or when rx_len bytes are ready).
// Returns: number of bytes in tx_buf to transmit, or 0 if no response.
uint16_t modbus_rtu_process(modbus_rtu_t *ctx);

// Reset the receive buffer (call on frame timeout / new frame start).
void modbus_rtu_rx_reset(modbus_rtu_t *ctx);

// Compute Modbus CRC-16 (polynomial 0xA001).
uint16_t modbus_crc16(const uint8_t *data, uint16_t len);

#endif // MODBUS_RTU_H
