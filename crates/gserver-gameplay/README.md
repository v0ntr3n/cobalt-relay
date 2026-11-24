# `gserver-gameplay` — Pure-Rust Gameplay & GM Engine for GServer (`vnsg2`)

Zero-dependency, `#![forbid(unsafe_code)]` Rust crate implementing the audited client-server gameplay RPCs, live server-push DTOs, `MainTown` / multiplayer walking town manager, in-game GM slash-command processor, Mail & Reward Center, Tavern Gacha (`raffle.*`) & Shop (`shop.*`), Turn-Based Battle Simulator (`FightProxy` / `BaseUnit`), Arena (`arena.*`), and Daily Sign-In / Online Rewards (`sign.*` / `online.*`) for **GServer (`vnsg2`)**.

## Audited Client Protocol Sources (Google Drive)

- [`client_protocol_refs.zip`](https://drive.google.com/file/d/1hs3EEuxJ8FP9Cdj6F07HrDJTvK0VEuYS/view?usp=drivesdk) (`BtDecryptHook.cpp`, `07_bt_decrypt_keys.inc`, `LCPacket.h`, `Amf3Module.cpp`, `live_capture_filtered.log`)
- [`NetConst.lua`](https://drive.google.com/file/d/13qIrtqC3DNqO365aYhLuqUzbYJuoWlG6/view?usp=drivesdk) & [`Network.lua`](https://drive.google.com/file/d/1ChHSQU91obK4GwKT1nDhgpE-hjRdUBaw/view?usp=drivesdk)
- [`MainScene.lua`](https://drive.google.com/file/d/1ZC4gmNW8JbmEvPHVQEoRXBhng4OhGXAh/view?usp=drivesdk), [`MainProxy.lua`](https://drive.google.com/file/d/1ln7dvlu2t0qPlJezRbuTz_KQjuzVujka/view?usp=drivesdk), [`MainTown.lua`](https://drive.google.com/file/d/1vCLzZtbYaOYTLFOMny4zYaZ7e2GpDCaJ/view?usp=drivesdk), [`RoyalCityHero.lua`](https://drive.google.com/file/d/1-c0L6lwdK6k6Bm_e2wlmzZERalJ3QNUE/view?usp=drivesdk), [`town.db`](https://drive.google.com/file/d/1J24MFvSpkE6sjaida0pLu4SwL6bi9yG0/view?usp=drivesdk)
- [`ChatProxy.lua`](https://drive.google.com/file/d/1Z2ZDqUkSTPZaEHBa-x_rLExY_Ve7UJ7n/view?usp=drivesdk), [`ChatConst.lua`](https://drive.google.com/file/d/1uYYsE7BpfcaMLnhUa_uIQzwhI5OtlRQu/view?usp=drivesdk), [`UserData.lua`](https://drive.google.com/file/d/1bXNJriCvHvmVDh4e8zSKs2F3g7HXKcig/view?usp=drivesdk), [`BagData.lua`](https://drive.google.com/file/d/1rhAPxkgYrMq0Cyuh7Ud6CnAFLlqCtNYI/view?usp=drivesdk), [`HerosData.lua`](https://drive.google.com/file/d/1reLYZx5UEMIyLyq4vofAlbGdhSAPemHg/view?usp=drivesdk)
- [`RewardData.lua`](https://drive.google.com/file/d/1FDYlZ-EnJd_6XUFhksLyZeO1KNe3g4M5/view?usp=drivesdk), [`RewardPanel.lua`](https://drive.google.com/file/d/1vrHB5SfPyEKOHoREt1KM_YsvKAqbHj0D/view?usp=drivesdk), [`mail_tmpl.db`](https://drive.google.com/file/d/1BDVcqaT5ocLp7z2Gz2eGeYbDY3LvTr0d/view?usp=drivesdk)
- [`RaffleGetPanel.lua`](https://drive.google.com/file/d/1HC4Nw3SqRuX50ULzjUg_ARhuhkzQjLEi/view?usp=drivesdk), [`RaffleVipPanel.lua`](https://drive.google.com/file/d/1MlVJZpyXqaNvnj6JZL7XMq7Od_05aoLJ/view?usp=drivesdk), [`raffle.db`](https://drive.google.com/file/d/1rU3LGwvuB5u23gb1_wDyY4ffSYlXHgqG/view?usp=drivesdk), [`BaseShopProxy.lua`](https://drive.google.com/file/d/1zXZaRqo3a4laVf4horoTWeLGNazcC0ls/view?usp=drivesdk), [`BaseShopData.lua`](https://drive.google.com/file/d/1tPQBxvBC-gbsNil5jTI8r0Pzg8C_WBfE/view?usp=drivesdk)
- [`CopyProxy.lua`](https://drive.google.com/file/d/1MXAiZEeOewFTQ6s_QE1nzA7_3oMMJOX0/view?usp=drivesdk), [`CopyData.lua`](https://drive.google.com/file/d/1xPdNBFvN0P84WuQDAsprdHFhNRyE650y/view?usp=drivesdk), [`CopyFightProxy.lua`](https://drive.google.com/file/d/1gIt_N-86UKXZpbWNGmKdZJHUmRggtDGe/view?usp=drivesdk), [`FightProxy.lua`](https://drive.google.com/file/d/1ouB5rMjGiYWwFsqlrYrI4pYC3R4DTUA4/view?usp=drivesdk), [`FightOperateLogic.lua`](https://drive.google.com/file/d/1VyfcL15lwI56LBz1QJK_LRkwpKZJK1Nx/view?usp=drivesdk), [`BaseUnit.lua`](https://drive.google.com/file/d/1GOXPOI8VbHaVaLDMe53i1UZ2D6u7bzaU/view?usp=drivesdk), [`PropertyKey.lua`](https://drive.google.com/file/d/15jwt1Ri3utVuQCpZWyDKydVPJOff4TmW/view?usp=drivesdk)
- [`ArenaProxy.lua`](https://drive.google.com/file/d/1vzHxjCcp41tvOjT68FJ-7_7EAoZX_NbP/view?usp=drivesdk), [`ActivitySignInCell.lua`](https://drive.google.com/file/d/1YUG8A2lnfomldsxIDl-mhzokbVdZpm76/view?usp=drivesdk), [`online.db`](https://drive.google.com/file/d/1Tfq4CYzC8qEBxz_cuXUe2q3Gc-tdvV9n/view?usp=drivesdk)

## Build & Offline Verification

```bash
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```
