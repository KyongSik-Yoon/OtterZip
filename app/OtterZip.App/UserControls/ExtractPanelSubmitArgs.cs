namespace OtterZip.App.UserControls;

/// <summary>
/// Result payload published by <see cref="ExtractPanel.Submitted"/>.
/// Lives in its own file (MA0048) and outside the host class (CA1034).
/// <c>Password</c> is null when the panel wasn't surfacing a password
/// field (archive isn't encrypted). <c>ExtractHere</c> tells the host
/// to ignore the supplied destination and use a sibling subfolder
/// instead — set when the user clicks the "여기에 풀기" button.
/// </summary>
public sealed record ExtractPanelSubmitArgs(string Destination, string? Password, bool ExtractHere);
