# Invoke-ScriptAnalyzer -Path install.ps1 -Settings ./PSScriptAnalyzerSettings.psd1
@{
    ExcludeRules = @(
        'PSAvoidUsingWriteHost',            # the installer talks to a person in a console
        'PSAvoidUsingEmptyCatchBlock',      # shutdown and health probes are best-effort on purpose
        'PSUseShouldProcessForStateChangingFunctions'  # Stop-Tray is internal to the installer
    )
}
