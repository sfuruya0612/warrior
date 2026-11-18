# ADR-019: 認証を Google OAuth とパスキーで行い、WebAuthn の検証を自前で実装する

- Status: Accepted

## Context

利用者は Google アカウントで登録し、ログインして利用する (REQ-NFR-006)。
ログインの後にパスキーを登録でき、登録した後はパスキーでもログインできる。
1 つのアカウントに複数のパスキーを登録でき、追加と削除ができる (REQ-NFR-013)。
パスキーを使えなくなった場合は、Google アカウントでログインして登録し直せる (REQ-NFR-014)。
参照する `../../../brewbook` は、パスキーだけの認証を、外部の WebAuthn クレートを使わずに自前で実装している。
`../../../brewbook` は `p256` と `sha2` で ES256 の署名を検証し、CBOR と COSE の必要な範囲を自前でデコードする。
`webauthn-rs` は wasm32 でビルドできない見込みであり、`../../../brewbook` は自前の実装を選んだ。
`../../../brewbook` の認証のコードは、登録、ログイン、チャレンジ、セッション、パスキーの管理を含み、テストを持っている。

Google の ID トークンの署名の検証には、wasm32 で動く RSA の実装が必要である。
AWS の `aws-lc-rs` は C/C++ コンパイラを要求し、Google の `ring` は wasm32-unknown-unknown でビルドできない既知の問題がある。
純 Rust の `rsa` は依存が多い。

## Decision

アカウントとパスキーの方針は ADR-013 に従う。
この ADR は Google OAuth と WebAuthn の検証の実装を定める。

Google のログインは OAuth 2.0 の認可コードフローで行う。
認可の URL、トークンの URL、UserInfo の URL は、環境ごとの vars で差し替えられるようにする。
テストでは、テスト専用の OIDC の提供者を指す。
状態、nonce、コード検証子を D1 に保存し、有効期限を 5 分、使用は 1 回とする。
認可コードはサーバー間で Google のトークンエンドポイントと交換する。
サーバーは ID トークンの署名を検証しない。
ID トークンはクライアントから受け取らず、サーバー間の TLS で Google のトークンエンドポイントから直接受け取るためである。
サーバーは ID トークンの payload をデコードし、`aud`、`iss`、`exp`、`nonce` を検証する。
利用者の情報は Google の UserInfo エンドポイントから取得し、`sub` と `email_verified` を検証する。
ID トークンの `sub` と UserInfo の `sub` が一致することを確認する。
JWT の署名検証ライブラリは追加しない。

WebAuthn の登録とログインでは、公開鍵のアルゴリズムを ES256 だけ受け付ける。
登録では `residentKey: preferred`、`userVerification: required`、`attestation: none` を要求し、ログインでも `userVerification: required` とする。
attestation statement は検証しない。
チャレンジは 32 バイトの乱数とし、D1 に保存し、有効期限は 5 分、使用は 1 回とする。
署名カウンタは W3C WebAuthn Level 3 の 7.2 節に従って検証する。

WebAuthn の検証は、`../../../brewbook` の `webauthn`、`cbor`、`cose`、`base64url` の実装を移植して使う。
登録とログインの経路は、パスキーだけの `../../../brewbook` の実装を、Google で作ったアカウントに合わせて変える。
セッションは 32 バイトの乱数とし、D1 には SHA-256 のハッシュだけを保存する。
セッションは HttpOnly、Secure、SameSite=Lax の Cookie で渡し、有効期限は発行から 30 日とする。
ただし、ローカル開発の `http://localhost` では Safari が Secure の Cookie を保存しないため、`ORIGIN` が HTTP のときはセッション、OAuth の state、再認証の Cookie に Secure を付けない。本番の HTTPS では必ず付ける。
ログアウトではセッションの行を物理削除する。
端末トークンは利用者に紐づけたまま保持し、別のアカウントへの切り替えで破棄する。
アカウントの削除では、セッションと端末トークンの行も削除する。

アカウントの削除の対象は ADR-013 に従う。
D1 の削除は `batch()` で行い、R2 の `users/<利用者 ID>/` のオブジェクトも削除する。
未処理のプッシュの予約と AI の Queue の対象を無効にする。
消費側は、利用者の行が無いメッセージを処理せずに捨てる。
認証の経路のレート制限は、ADR-016 の規則に従う。

## Consequences

WebAuthn の検証を自前で実装するため、仕様への準拠とテストの厚さが安全性を決める。
`../../../brewbook` の実装とテストを移植し、仕様の該当節を引用したコメントを付ける。

Google の ID トークンの署名を検証しないため、ID トークンをクライアントから受け取る経路を追加してはならない。
クライアントから ID トークンを受け取る方式に変える場合は、署名の検証を追加する ADR を起こす。

AWS の `aws-lc-rs` は C/C++ コンパイラを要求し、Google の `ring` は wasm32-unknown-unknown でビルドできない既知の問題がある。
どちらも Workers の wasm のビルドに載せない。
純 Rust の `rsa` は依存が多いため、署名の検証が必要になった場合は、依存と wasm のビルドを比較して別の ADR で決める。

Relying Party ID は Worker のホスト名になる。
ホスト名を変えると、登録済みのパスキーが無効になり、Google OAuth のリダイレクト URI も変わる。
ホスト名を固定する。

`../../../brewbook` の認証のコードは、パスキーだけの前提で書かれている。
Google のアカウントとの紐づけ、パスキーの追加の条件、アカウントの削除の範囲を、移植のときに変える。

## References

- `../../../brewbook/docs/adr/0004-passkey-only-authentication.md`
- `../../../brewbook/backend/brew_book_core/src/webauthn.rs`
- `../../../brewbook/backend/brew_book/src/auth/`
- `docs/prd/warrior.md` の「マイページ」「非機能要件」
- Google Identity の OAuth 2.0 と OpenID Connect の UserInfo のドキュメント
- `ring` の wasm32-unknown-unknown のビルドの問題 (GitHub issue 1847)
- AWS-LC for Rust のプラットフォーム対応のドキュメント
- W3C WebAuthn Level 3
