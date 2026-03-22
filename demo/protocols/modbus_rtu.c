// SPDX-License-Identifier: MPL-2.0
// Modbus RTU Slave — minimal bare-metal implementation
//
// Implements:
//   FC01 — Read Coils
//   FC03 — Read Holding Registers
//   FC06 — Write Single Register
//   FC16 — Write Multiple Registers
//
// Frame format: [slave_addr:1][function:1][data:N][crc16_lo:1][crc16_hi:1]
// CRC-16 uses Modbus polynomial 0xA001 (bit-reversed 0x8005).
//
// No dynamic allocation, no OS, no libc required.

#include "modbus_rtu.h"

// ── CRC-16 (Modbus) ──
// Standard Modbus CRC using the 0xA001 polynomial.
// This is a byte-at-a-time implementation suitable for bare metal.

uint16_t modbus_crc16(const uint8_t *data, uint16_t len) {
    uint16_t crc = 0xFFFF;
    for (uint16_t i = 0; i < len; i++) {
        crc ^= (uint16_t)data[i];
        for (uint8_t bit = 0; bit < 8; bit++) {
            if (crc & 0x0001) {
                crc >>= 1;
                crc ^= 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    return crc;
}

// ── Init ──

void modbus_rtu_init(modbus_rtu_t *ctx, uint8_t slave_addr,
                     modbus_read_reg_fn read_reg,
                     modbus_write_reg_fn write_reg,
                     modbus_read_coil_fn read_coil,
                     void *user_ctx) {
    ctx->slave_addr = slave_addr;
    ctx->rx_len = 0;
    ctx->tx_len = 0;
    ctx->read_reg = read_reg;
    ctx->write_reg = write_reg;
    ctx->read_coil = read_coil;
    ctx->user_ctx = user_ctx;
    ctx->rx_count = 0;
    ctx->tx_count = 0;
    ctx->err_count = 0;
}

// ── Receive ──

void modbus_rtu_rx_byte(modbus_rtu_t *ctx, uint8_t byte) {
    if (ctx->rx_len < MODBUS_MAX_PDU_SIZE) {
        ctx->rx_buf[ctx->rx_len++] = byte;
    }
}

void modbus_rtu_rx_reset(modbus_rtu_t *ctx) {
    ctx->rx_len = 0;
}

// ── Internal: build exception response ──

static uint16_t modbus_exception(modbus_rtu_t *ctx, uint8_t fc, uint8_t exc) {
    ctx->tx_buf[0] = ctx->slave_addr;
    ctx->tx_buf[1] = fc | 0x80;  // Error flag
    ctx->tx_buf[2] = exc;
    uint16_t crc = modbus_crc16(ctx->tx_buf, 3);
    ctx->tx_buf[3] = (uint8_t)(crc & 0xFF);
    ctx->tx_buf[4] = (uint8_t)(crc >> 8);
    ctx->tx_len = 5;
    ctx->err_count++;
    return 5;
}

// ── FC01: Read Coils ──

static uint16_t modbus_fc01_read_coils(modbus_rtu_t *ctx) {
    if (ctx->rx_len < 8) {
        return modbus_exception(ctx, MODBUS_FC_READ_COILS, MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    uint16_t start_addr = ((uint16_t)ctx->rx_buf[2] << 8) | ctx->rx_buf[3];
    uint16_t quantity   = ((uint16_t)ctx->rx_buf[4] << 8) | ctx->rx_buf[5];

    if (quantity == 0 || quantity > 2000) {
        return modbus_exception(ctx, MODBUS_FC_READ_COILS, MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    // Number of bytes needed for the coil data
    uint8_t byte_count = (uint8_t)((quantity + 7) / 8);

    // Build response header
    ctx->tx_buf[0] = ctx->slave_addr;
    ctx->tx_buf[1] = MODBUS_FC_READ_COILS;
    ctx->tx_buf[2] = byte_count;

    // Clear coil data bytes
    for (uint8_t i = 0; i < byte_count; i++) {
        ctx->tx_buf[3 + i] = 0;
    }

    // Read each coil via callback
    for (uint16_t i = 0; i < quantity; i++) {
        uint8_t coil_val = 0;
        if (ctx->read_coil) {
            int rc = ctx->read_coil(start_addr + i, &coil_val, ctx->user_ctx);
            if (rc != 0) {
                return modbus_exception(ctx, MODBUS_FC_READ_COILS,
                                        MODBUS_EX_ILLEGAL_DATA_ADDRESS);
            }
        }
        if (coil_val) {
            ctx->tx_buf[3 + (i / 8)] |= (uint8_t)(1 << (i % 8));
        }
    }

    uint16_t resp_len = 3 + byte_count;
    uint16_t crc = modbus_crc16(ctx->tx_buf, resp_len);
    ctx->tx_buf[resp_len]     = (uint8_t)(crc & 0xFF);
    ctx->tx_buf[resp_len + 1] = (uint8_t)(crc >> 8);
    ctx->tx_len = resp_len + 2;
    return ctx->tx_len;
}

// ── FC03: Read Holding Registers ──

static uint16_t modbus_fc03_read_holding_regs(modbus_rtu_t *ctx) {
    if (ctx->rx_len < 8) {
        return modbus_exception(ctx, MODBUS_FC_READ_HOLDING_REGS,
                                MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    uint16_t start_addr = ((uint16_t)ctx->rx_buf[2] << 8) | ctx->rx_buf[3];
    uint16_t quantity   = ((uint16_t)ctx->rx_buf[4] << 8) | ctx->rx_buf[5];

    if (quantity == 0 || quantity > 125) {
        return modbus_exception(ctx, MODBUS_FC_READ_HOLDING_REGS,
                                MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    // Build response
    ctx->tx_buf[0] = ctx->slave_addr;
    ctx->tx_buf[1] = MODBUS_FC_READ_HOLDING_REGS;
    ctx->tx_buf[2] = (uint8_t)(quantity * 2);

    for (uint16_t i = 0; i < quantity; i++) {
        uint16_t val = 0;
        int rc = 0;
        if (ctx->read_reg) {
            rc = ctx->read_reg(start_addr + i, &val, ctx->user_ctx);
        } else {
            rc = -1;
        }
        if (rc != 0) {
            return modbus_exception(ctx, MODBUS_FC_READ_HOLDING_REGS,
                                    MODBUS_EX_ILLEGAL_DATA_ADDRESS);
        }
        ctx->tx_buf[3 + i * 2]     = (uint8_t)(val >> 8);
        ctx->tx_buf[3 + i * 2 + 1] = (uint8_t)(val & 0xFF);
    }

    uint16_t resp_len = 3 + quantity * 2;
    uint16_t crc = modbus_crc16(ctx->tx_buf, resp_len);
    ctx->tx_buf[resp_len]     = (uint8_t)(crc & 0xFF);
    ctx->tx_buf[resp_len + 1] = (uint8_t)(crc >> 8);
    ctx->tx_len = resp_len + 2;
    return ctx->tx_len;
}

// ── FC06: Write Single Register ──

static uint16_t modbus_fc06_write_single_reg(modbus_rtu_t *ctx) {
    if (ctx->rx_len < 8) {
        return modbus_exception(ctx, MODBUS_FC_WRITE_SINGLE_REG,
                                MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    uint16_t reg_addr = ((uint16_t)ctx->rx_buf[2] << 8) | ctx->rx_buf[3];
    uint16_t value    = ((uint16_t)ctx->rx_buf[4] << 8) | ctx->rx_buf[5];

    int rc = 0;
    if (ctx->write_reg) {
        rc = ctx->write_reg(reg_addr, value, ctx->user_ctx);
    } else {
        rc = -1;
    }

    if (rc != 0) {
        return modbus_exception(ctx, MODBUS_FC_WRITE_SINGLE_REG,
                                MODBUS_EX_ILLEGAL_DATA_ADDRESS);
    }

    // Echo the request back (standard FC06 response)
    ctx->tx_buf[0] = ctx->slave_addr;
    ctx->tx_buf[1] = MODBUS_FC_WRITE_SINGLE_REG;
    ctx->tx_buf[2] = (uint8_t)(reg_addr >> 8);
    ctx->tx_buf[3] = (uint8_t)(reg_addr & 0xFF);
    ctx->tx_buf[4] = (uint8_t)(value >> 8);
    ctx->tx_buf[5] = (uint8_t)(value & 0xFF);
    uint16_t crc = modbus_crc16(ctx->tx_buf, 6);
    ctx->tx_buf[6] = (uint8_t)(crc & 0xFF);
    ctx->tx_buf[7] = (uint8_t)(crc >> 8);
    ctx->tx_len = 8;
    return 8;
}

// ── FC16: Write Multiple Registers ──

static uint16_t modbus_fc16_write_multiple_regs(modbus_rtu_t *ctx) {
    if (ctx->rx_len < 9) {
        return modbus_exception(ctx, MODBUS_FC_WRITE_MULTIPLE_REGS,
                                MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    uint16_t start_addr = ((uint16_t)ctx->rx_buf[2] << 8) | ctx->rx_buf[3];
    uint16_t quantity   = ((uint16_t)ctx->rx_buf[4] << 8) | ctx->rx_buf[5];
    uint8_t  byte_count = ctx->rx_buf[6];

    if (quantity == 0 || quantity > 123 || byte_count != quantity * 2) {
        return modbus_exception(ctx, MODBUS_FC_WRITE_MULTIPLE_REGS,
                                MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    // Verify we have enough data: 7 header + byte_count data + 2 CRC
    if (ctx->rx_len < (uint16_t)(7 + byte_count + 2)) {
        return modbus_exception(ctx, MODBUS_FC_WRITE_MULTIPLE_REGS,
                                MODBUS_EX_ILLEGAL_DATA_VALUE);
    }

    // Write each register
    for (uint16_t i = 0; i < quantity; i++) {
        uint16_t val = ((uint16_t)ctx->rx_buf[7 + i * 2] << 8) |
                       ctx->rx_buf[7 + i * 2 + 1];
        int rc = 0;
        if (ctx->write_reg) {
            rc = ctx->write_reg(start_addr + i, val, ctx->user_ctx);
        } else {
            rc = -1;
        }
        if (rc != 0) {
            return modbus_exception(ctx, MODBUS_FC_WRITE_MULTIPLE_REGS,
                                    MODBUS_EX_ILLEGAL_DATA_ADDRESS);
        }
    }

    // Response: slave_addr, fc, start_addr (2), quantity (2), crc (2)
    ctx->tx_buf[0] = ctx->slave_addr;
    ctx->tx_buf[1] = MODBUS_FC_WRITE_MULTIPLE_REGS;
    ctx->tx_buf[2] = (uint8_t)(start_addr >> 8);
    ctx->tx_buf[3] = (uint8_t)(start_addr & 0xFF);
    ctx->tx_buf[4] = (uint8_t)(quantity >> 8);
    ctx->tx_buf[5] = (uint8_t)(quantity & 0xFF);
    uint16_t crc = modbus_crc16(ctx->tx_buf, 6);
    ctx->tx_buf[6] = (uint8_t)(crc & 0xFF);
    ctx->tx_buf[7] = (uint8_t)(crc >> 8);
    ctx->tx_len = 8;
    return 8;
}

// ── Process a complete Modbus RTU frame ──

uint16_t modbus_rtu_process(modbus_rtu_t *ctx) {
    ctx->tx_len = 0;

    // Minimum frame: addr(1) + fc(1) + data(min 2) + crc(2) = 6 bytes
    // But some FCs need at least 8. We check per-FC.
    if (ctx->rx_len < 4) {
        // Too short to be any valid frame
        return 0;
    }

    // Check slave address — respond only to our address or broadcast (0)
    uint8_t addr = ctx->rx_buf[0];
    if (addr != ctx->slave_addr && addr != 0) {
        return 0;  // Not for us, ignore silently
    }

    // Verify CRC
    uint16_t received_crc = ((uint16_t)ctx->rx_buf[ctx->rx_len - 1] << 8) |
                            ctx->rx_buf[ctx->rx_len - 2];
    uint16_t calc_crc = modbus_crc16(ctx->rx_buf, ctx->rx_len - 2);
    if (received_crc != calc_crc) {
        ctx->err_count++;
        return 0;  // CRC error — no response per Modbus spec
    }

    ctx->rx_count++;

    // Don't respond to broadcast writes
    uint8_t fc = ctx->rx_buf[1];
    uint16_t result = 0;

    switch (fc) {
        case MODBUS_FC_READ_COILS:
            result = modbus_fc01_read_coils(ctx);
            break;
        case MODBUS_FC_READ_HOLDING_REGS:
            result = modbus_fc03_read_holding_regs(ctx);
            break;
        case MODBUS_FC_WRITE_SINGLE_REG:
            result = modbus_fc06_write_single_reg(ctx);
            break;
        case MODBUS_FC_WRITE_MULTIPLE_REGS:
            result = modbus_fc16_write_multiple_regs(ctx);
            break;
        default:
            result = modbus_exception(ctx, fc, MODBUS_EX_ILLEGAL_FUNCTION);
            break;
    }

    // Don't respond to broadcast (address 0)
    if (addr == 0) {
        ctx->tx_len = 0;
        return 0;
    }

    ctx->tx_count++;
    return result;
}
