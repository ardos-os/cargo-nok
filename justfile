# Garante que o just usa a tua bash/zsh padrão e respeita o ambiente do direnv
set shell := ["bash", "-c"]

# Atalho para compilar o plugin e testá-lo imediatamente
[private]
default:
    @just --list

# Compila o cargo-nok e executa-o passando quaisquer argumentos adiante
build:
    cargo build