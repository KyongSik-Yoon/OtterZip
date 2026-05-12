using System.Collections.Generic;
using System.Collections.Specialized;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Models;
using OtterZip.App.Services;

namespace OtterZip.App.UserControls;

/// <summary>
/// Hosts the floating <see cref="JobCard"/> stack tied to a
/// <see cref="JobQueue"/>. Subscribes to the queue's
/// <c>ObservableCollection</c> and adds/removes <see cref="JobCard"/>
/// instances accordingly — no ItemsControl DataTemplate magic needed,
/// which keeps the path simple under AOT.
///
/// The layer is fully transparent and click-through except for the
/// cards themselves; the underlying canvas keeps receiving pointer
/// events around the cards.
/// </summary>
public sealed partial class FloatLayer : UserControl
{
    private JobQueue? _queue;
    private readonly Dictionary<JobItem, JobCard> _cards = new();

    public FloatLayer()
    {
        InitializeComponent();
    }

    /// <summary>
    /// Wire this layer to a JobQueue. Called once from MainWindow's
    /// constructor after the queue is created. Idempotent — re-binding
    /// detaches the previous queue cleanly.
    /// </summary>
    public void Attach(JobQueue queue)
    {
        if (_queue is not null)
        {
            _queue.Jobs.CollectionChanged -= OnJobsChanged;
            CardsHost.Items.Clear();
            _cards.Clear();
        }
        _queue = queue;
        _queue.Jobs.CollectionChanged += OnJobsChanged;
        // Seed any pre-existing items (defensive — usually empty here).
        foreach (var item in _queue.Jobs)
        {
            AddCard(item);
        }
    }

    private void OnJobsChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        switch (e.Action)
        {
            case NotifyCollectionChangedAction.Add:
                if (e.NewItems is not null)
                {
                    foreach (JobItem item in e.NewItems)
                    {
                        AddCard(item);
                    }
                }
                break;
            case NotifyCollectionChangedAction.Remove:
                if (e.OldItems is not null)
                {
                    foreach (JobItem item in e.OldItems)
                    {
                        RemoveCard(item);
                    }
                }
                break;
            case NotifyCollectionChangedAction.Reset:
                CardsHost.Items.Clear();
                _cards.Clear();
                break;
        }
    }

    private void AddCard(JobItem item)
    {
        if (_queue is null) return;
        if (_cards.ContainsKey(item)) return;
        var card = new JobCard();
        card.Bind(item, _queue);
        _cards[item] = card;
        CardsHost.Items.Add(card);
    }

    private void RemoveCard(JobItem item)
    {
        if (_cards.TryGetValue(item, out var card))
        {
            CardsHost.Items.Remove(card);
            _cards.Remove(item);
        }
    }
}
