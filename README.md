# Logitech Mouse DPI

A tiny Linux app to change the DPI of your Logitech mouse.

![icon](packaging/logitech-dpi.svg)

## Install

Open a terminal and paste:

```bash
curl -fsSL https://raw.githubusercontent.com/S0CloseYetS0Far/Logitech-Mouse-DPI/main/install.sh | bash
```

It asks for your password once (so the app can talk to the mouse without sudo).
Then open **Logitech Mouse DPI** from your app menu.

## Use

Move the slider, press **Apply**. That's it.

If the window says it needs permission, press **Allow access** and enter your password.

## Supported mice

Most Logitech mice with adjustable DPI: wired, wireless (Unifying, Lightspeed,
Bolt receivers) and Bluetooth. Older models (G5, G9, G9x, G500, G500s, G700,
G700s) are supported too.

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/S0CloseYetS0Far/Logitech-Mouse-DPI/main/install.sh | bash -s -- --uninstall
```

## Build from source

Needs [Rust](https://rustup.rs). Then:

```bash
git clone https://github.com/S0CloseYetS0Far/Logitech-Mouse-DPI.git
cd Logitech-Mouse-DPI
./install.sh
```

Protocol details come from [libratbag](https://github.com/libratbag/libratbag) and [Solaar](https://github.com/pwr-Solaar/Solaar).
