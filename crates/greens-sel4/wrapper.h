// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Markku Ahvenjärvi
#include <sel4/sel4_virt.h>

const unsigned int RPC_ADDRESS_SPACE_GLOBAL = AS_GLOBAL;

static inline unsigned int rpc_op(seL4_Word mr0)
{
	return QEMU_OP(mr0);
}

static inline unsigned int rpc_ioreq_direction(seL4_Word mr0)
{
	return BIT_FIELD_GET(mr0, RPC_MR0_MMIO_DIRECTION);
}

static inline unsigned int rpc_ioreq_address_space(seL4_Word mr0)
{
	return BIT_FIELD_GET(mr0, RPC_MR0_MMIO_ADDR_SPACE);
}

static inline unsigned int rpc_ioreq_len(seL4_Word mr0)
{
	return BIT_FIELD_GET(mr0, RPC_MR0_MMIO_LENGTH);
}

static inline unsigned int rpc_ioreq_slot(seL4_Word mr0)
{
	return BIT_FIELD_GET(mr0, RPC_MR0_MMIO_SLOT);
}

