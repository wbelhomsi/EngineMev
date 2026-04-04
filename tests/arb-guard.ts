import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { ArbGuard } from "../target/types/arb_guard";
import {
  createMint,
  createAccount,
  mintTo,
  transfer,
  TOKEN_PROGRAM_ID,
} from "@solana/spl-token";
import { assert } from "chai";

describe("arb-guard", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);
  const program = anchor.workspace.ArbGuard as Program<ArbGuard>;
  const authority = provider.wallet.publicKey;

  let mint: anchor.web3.PublicKey;
  let tokenAccount: anchor.web3.PublicKey;
  let guardState: anchor.web3.PublicKey;

  before(async () => {
    // Create a test token and fund it
    mint = await createMint(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      authority,
      null,
      9
    );
    tokenAccount = await createAccount(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      authority
    );
    await mintTo(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      tokenAccount,
      authority,
      1_000_000_000
    );

    // Derive guard PDA
    [guardState] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("guard"), authority.toBuffer()],
      program.programId
    );
  });

  it("start_check records balance", async () => {
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    const state = await program.account.guardState.fetch(guardState);
    assert.equal(state.startBalance.toNumber(), 1_000_000_000);
    assert.ok(state.authority.equals(authority));
  });

  it("profit_check succeeds when balance increased", async () => {
    // Mint more tokens (simulating swap profit)
    await mintTo(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      tokenAccount,
      authority,
      100_000
    );

    await program.methods
      .profitCheck(new anchor.BN(0))
      .accounts({ authority, guardState, tokenAccount })
      .rpc();
    // Should not throw
  });

  it("profit_check reverts when balance decreased", async () => {
    // Record current balance
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    // Transfer tokens out (simulating loss)
    const recipient = await createAccount(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      anchor.web3.Keypair.generate().publicKey
    );
    await transfer(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      tokenAccount,
      recipient,
      (provider.wallet as anchor.Wallet).payer,
      500_000_000
    );

    // profit_check should fail
    try {
      await program.methods
        .profitCheck(new anchor.BN(0))
        .accounts({ authority, guardState, tokenAccount })
        .rpc();
      assert.fail("Should have reverted");
    } catch (err: any) {
      assert.include(err.message, "NoProfitDetected");
    }
  });

  it("profit_check enforces min_profit", async () => {
    // Reset: mint tokens back up
    await mintTo(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      tokenAccount,
      authority,
      500_000_000
    );

    // Record balance
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    // Add small profit (100 lamports)
    await mintTo(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      tokenAccount,
      authority,
      100
    );

    // min_profit=50 should succeed
    await program.methods
      .profitCheck(new anchor.BN(50))
      .accounts({ authority, guardState, tokenAccount })
      .rpc();

    // Reset and try min_profit=200 (more than we added)
    await program.methods
      .startCheck()
      .accounts({
        authority,
        guardState,
        tokenAccount,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    await mintTo(
      provider.connection,
      (provider.wallet as anchor.Wallet).payer,
      mint,
      tokenAccount,
      authority,
      100
    );

    try {
      await program.methods
        .profitCheck(new anchor.BN(200))
        .accounts({ authority, guardState, tokenAccount })
        .rpc();
      assert.fail("Should have reverted -- profit below min");
    } catch (err: any) {
      assert.include(err.message, "NoProfitDetected");
    }
  });
});
